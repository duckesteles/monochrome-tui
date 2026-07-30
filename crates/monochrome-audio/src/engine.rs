use crate::convert::{LinearResampler, map_channels, replay_gain_scale};
use crate::ring::Ring;
use crate::source::{self, ByteRange, HttpRange, RangeSource};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{Arc, mpsc};
use std::time::Duration;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

const RING_SECONDS: f32 = 4.0;
const IDLE_SLEEP: Duration = Duration::from_millis(5);

#[derive(Debug, Clone)]
pub struct PlayRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub replay_gain: Option<f32>,
    pub peak: Option<f32>,
    pub decryption_key: Option<String>,
}

#[derive(Debug)]
pub enum Command {
    Play(Box<PlayRequest>),
    Pause,
    Resume,
    Stop,
    SeekTo(f64),
    SetVolume(f32),
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum Event {
    Loading,
    Started {
        duration: Option<f64>,
        sample_rate: u32,
        channels: u16,
        codec: String,
    },
    Position(f64),
    Output {
        sample_rate: u32,
        channels: u16,
        resampling: bool,
    },
    Paused(bool),
    Finished,
    Stopped,
    Failed(String),
}

struct Shared {
    ring: Ring,
    playing: AtomicBool,
    volume: AtomicU32,
    gain: AtomicU32,
    frames: AtomicU64,
    output_channels: AtomicU32,
}

impl Shared {
    fn new(ring_samples: usize) -> Self {
        Self {
            ring: Ring::with_capacity(ring_samples),
            playing: AtomicBool::new(false),
            volume: AtomicU32::new(1.0f32.to_bits()),
            gain: AtomicU32::new(1.0f32.to_bits()),
            frames: AtomicU64::new(0),
            output_channels: AtomicU32::new(2),
        }
    }

    fn volume(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::Relaxed))
    }

    fn gain(&self) -> f32 {
        f32::from_bits(self.gain.load(Ordering::Relaxed))
    }
}

pub struct Player {
    commands: Sender<Command>,
    shared: Arc<Shared>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Player {
    pub fn spawn() -> (Self, Receiver<Event>) {
        let (commands, command_rx) = mpsc::channel();
        let (events, event_rx) = mpsc::channel();
        let shared = Arc::new(Shared::new((48_000.0 * 2.0 * RING_SECONDS) as usize));
        let worker_shared = Arc::clone(&shared);
        let worker = std::thread::Builder::new()
            .name("monochrome-audio".into())
            .spawn(move || run(command_rx, events, worker_shared))
            .expect("audio worker starts");

        (
            Self {
                commands,
                shared,
                worker: Some(worker),
            },
            event_rx,
        )
    }

    pub fn play(&self, request: PlayRequest) {
        let _ = self.commands.send(Command::Play(Box::new(request)));
    }

    pub fn pause(&self) {
        let _ = self.commands.send(Command::Pause);
    }

    pub fn resume(&self) {
        let _ = self.commands.send(Command::Resume);
    }

    pub fn stop(&self) {
        let _ = self.commands.send(Command::Stop);
    }

    pub fn seek_to(&self, seconds: f64) {
        let _ = self.commands.send(Command::SeekTo(seconds.max(0.0)));
    }

    pub fn set_volume(&self, volume: f32) {
        let clamped = volume.clamp(0.0, 1.0);
        self.shared
            .volume
            .store(clamped.to_bits(), Ordering::Relaxed);
        let _ = self.commands.send(Command::SetVolume(clamped));
    }

    pub fn volume(&self) -> f32 {
        self.shared.volume()
    }

    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::Relaxed)
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Output {
    _stream: cpal::Stream,
    sample_rate: u32,
    channels: usize,
}

fn build_output(
    shared: &Arc<Shared>,
    preferred_rate: u32,
    preferred_channels: u16,
) -> Result<Output, String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "no audio output device is available".to_string())?;

    let mut chosen = None;
    if let Ok(configs) = device.supported_output_configs() {
        let ranges: Vec<_> = configs.collect();
        for range in &ranges {
            if range.sample_format() == cpal::SampleFormat::F32
                && range.channels() == preferred_channels
                && range.min_sample_rate().0 <= preferred_rate
                && range.max_sample_rate().0 >= preferred_rate
            {
                chosen = Some(range.with_sample_rate(cpal::SampleRate(preferred_rate)));
                break;
            }
        }
        if chosen.is_none() {
            for range in &ranges {
                if range.sample_format() == cpal::SampleFormat::F32
                    && range.min_sample_rate().0 <= preferred_rate
                    && range.max_sample_rate().0 >= preferred_rate
                {
                    chosen = Some(range.with_sample_rate(cpal::SampleRate(preferred_rate)));
                    break;
                }
            }
        }
    }

    let config = match chosen {
        Some(config) => config,
        None => device
            .default_output_config()
            .map_err(|error| format!("no usable audio configuration: {error}"))?,
    };

    let sample_rate = config.sample_rate().0;
    let channels = config.channels() as usize;
    shared
        .output_channels
        .store(channels as u32, Ordering::Relaxed);

    let callback_shared = Arc::clone(shared);
    let stream = device
        .build_output_stream(
            &config.config(),
            move |output: &mut [f32], _: &cpal::OutputCallbackInfo| {
                if !callback_shared.playing.load(Ordering::Relaxed) {
                    output.fill(0.0);
                    return;
                }
                let filled = callback_shared.ring.pop(output);
                let level = callback_shared.volume() * callback_shared.gain();
                for sample in output[..filled].iter_mut() {
                    *sample *= level;
                }
                output[filled..].fill(0.0);
                let channels = callback_shared
                    .output_channels
                    .load(Ordering::Relaxed)
                    .max(1);
                callback_shared
                    .frames
                    .fetch_add((filled / channels as usize) as u64, Ordering::Relaxed);
            },
            move |error| tracing::warn!(%error, "audio output error"),
            None,
        )
        .map_err(|error| format!("cannot open the audio device: {error}"))?;

    stream
        .play()
        .map_err(|error| format!("cannot start the audio device: {error}"))?;

    Ok(Output {
        _stream: stream,
        sample_rate,
        channels,
    })
}

impl Playback {
    fn retune(&mut self, output_rate: u32, output_channels: usize) {
        self.resampler = LinearResampler::new(self.source_rate, output_rate, output_channels);
    }
}

struct Playback {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    track_id: u32,
    source_rate: u32,
    source_channels: usize,
    duration: Option<f64>,
    resampler: LinearResampler,
    finished: bool,
}

fn open(request: &PlayRequest, output: &Output) -> Result<Playback, String> {
    let backend = HttpRange::open(&request.url, &request.headers)
        .map_err(|error| format!("cannot reach the audio source: {error}"))?;

    let content_type = backend.content_type();
    let hint = match content_type.as_deref().and_then(source::extension_for) {
        Some(extension) => {
            let mut hint = Hint::new();
            hint.with_extension(extension);
            hint
        }
        None => Hint::new(),
    };

    if let Some(kind) = content_type
        .as_deref()
        .filter(|kind| source::is_textual(kind))
    {
        let detail = read_error_body(&backend);
        return Err(match detail {
            Some(detail) => format!("the source returned a message, not audio: {detail}"),
            None => format!("the source returned {kind}, not audio"),
        });
    }

    let source = RangeSource::new(Box::new(backend));

    let stream = match request.decryption_key.as_deref() {
        Some(hex) => {
            let key = crate::cenc::parse_key(hex)
                .ok_or_else(|| "the gateway sent an unusable decryption key".to_string())?;
            let decrypted = crate::cenc::FlacFromCenc::new(source, key);
            let buffered = crate::spill::Spill::new(decrypted)
                .map_err(|error| format!("cannot buffer the decrypted stream: {error}"))?;
            let mut flac_hint = Hint::new();
            flac_hint.with_extension("flac");
            return prepare(
                MediaSourceStream::new(Box::new(buffered), MediaSourceStreamOptions::default()),
                output.sample_rate,
                output.channels,
                flac_hint,
            );
        }
        None => MediaSourceStream::new(Box::new(source), MediaSourceStreamOptions::default()),
    };
    prepare(stream, output.sample_rate, output.channels, hint)
}

fn read_error_body(backend: &HttpRange) -> Option<String> {
    let mut reader = match backend.open_at(0) {
        Ok(reader) => reader,
        Err(error) => return Some(error.to_string()),
    };
    let mut body = String::new();
    let _ = std::io::Read::read_to_string(&mut std::io::Read::take(&mut reader, 800), &mut body);
    source::summarise(&body)
}

fn prepare(
    stream: MediaSourceStream,
    output_rate: u32,
    output_channels: usize,
    hint: Hint,
) -> Result<Playback, String> {
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions {
                enable_gapless: true,
                ..Default::default()
            },
            &MetadataOptions::default(),
        )
        .map_err(|error| {
            tracing::debug!(%error, "the probe could not identify the stream");
            "this stream is not audio the client can read. the gateway may have returned an \
             encrypted or fragmented file"
                .to_string()
        })?;

    let format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|track| track.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| "the stream carries no audio track".to_string())?;

    let track_id = track.id;
    let parameters = track.codec_params.clone();
    let decoder = symphonia::default::get_codecs()
        .make(&parameters, &DecoderOptions::default())
        .map_err(|error| format!("no decoder for this stream: {error}"))?;

    let source_rate = parameters.sample_rate.unwrap_or(output_rate);
    let source_channels = parameters
        .channels
        .map(|channels| channels.count())
        .unwrap_or(2);
    let duration = match (parameters.n_frames, parameters.sample_rate) {
        (Some(frames), Some(rate)) if rate > 0 => Some(frames as f64 / rate as f64),
        _ => None,
    };

    Ok(Playback {
        format,
        decoder,
        track_id,
        source_rate,
        source_channels,
        duration,
        resampler: LinearResampler::new(source_rate, output_rate, output_channels),
        finished: false,
    })
}

fn run(commands: Receiver<Command>, events: Sender<Event>, shared: Arc<Shared>) {
    let mut output: Option<Output> = None;
    let mut playback: Option<Playback> = None;
    let mut sample_buffer: Option<SampleBuffer<f32>> = None;
    let mut mapped: Vec<f32> = Vec::new();
    let mut resampled: Vec<f32> = Vec::new();
    let mut pending: Vec<f32> = Vec::new();
    let mut last_reported = f64::MIN;
    let mut seek_offset = 0.0f64;
    let mut current: Option<PlayRequest> = None;

    loop {
        let mut idle = true;

        loop {
            match commands.try_recv() {
                Ok(Command::Shutdown) | Err(TryRecvError::Disconnected) => {
                    shared.playing.store(false, Ordering::Relaxed);
                    return;
                }
                Ok(Command::Play(request)) => {
                    idle = false;
                    shared.playing.store(false, Ordering::Relaxed);
                    drop(playback.take());
                    shared.ring.clear();
                    shared.frames.store(0, Ordering::Relaxed);
                    pending.clear();
                    sample_buffer = None;
                    seek_offset = 0.0;
                    last_reported = f64::MIN;
                    current = Some((*request).clone());
                    let _ = events.send(Event::Loading);

                    shared.gain.store(
                        replay_gain_scale(request.replay_gain, request.peak).to_bits(),
                        Ordering::Relaxed,
                    );

                    let device = match output.take() {
                        Some(device) => Ok(device),
                        None => build_output(&shared, 48_000, 2),
                    };
                    let device = match device {
                        Ok(device) => device,
                        Err(error) => {
                            let _ = events.send(Event::Failed(error));
                            playback = None;
                            continue;
                        }
                    };

                    match open(&request, &device) {
                        Ok(mut opened) => {
                            let device = match rebuild_if_needed(&shared, device, &mut opened) {
                                Ok(device) => device,
                                Err(error) => {
                                    let _ = events.send(Event::Failed(error));
                                    playback = None;
                                    continue;
                                }
                            };
                            let _ = events.send(Event::Output {
                                sample_rate: device.sample_rate,
                                channels: device.channels as u16,
                                resampling: !opened.resampler.is_identity()
                                    || device.channels != opened.source_channels,
                            });
                            let _ = events.send(Event::Started {
                                duration: opened.duration,
                                sample_rate: opened.source_rate,
                                channels: opened.source_channels as u16,
                                codec: codec_name(&opened),
                            });
                            output = Some(device);
                            playback = Some(opened);
                            shared.playing.store(true, Ordering::Relaxed);
                        }
                        Err(error) => {
                            output = Some(device);
                            playback = None;
                            let _ = events.send(Event::Failed(error));
                        }
                    }
                }
                Ok(Command::Pause) => {
                    idle = false;
                    shared.playing.store(false, Ordering::Relaxed);
                    let _ = events.send(Event::Paused(true));
                }
                Ok(Command::Resume) => {
                    idle = false;
                    if playback.is_some() {
                        shared.playing.store(true, Ordering::Relaxed);
                        let _ = events.send(Event::Paused(false));
                    }
                }
                Ok(Command::Stop) => {
                    idle = false;
                    shared.playing.store(false, Ordering::Relaxed);
                    shared.ring.clear();
                    shared.frames.store(0, Ordering::Relaxed);
                    pending.clear();
                    playback = None;
                    current = None;
                    let _ = events.send(Event::Stopped);
                }
                Ok(Command::SeekTo(seconds)) => {
                    idle = false;
                    let seeked_in_place = playback
                        .as_mut()
                        .map(|active| seek_within(active, seconds))
                        .unwrap_or(false);

                    let landed = match seeked_in_place {
                        true => true,
                        false => match (current.as_ref(), output.as_ref()) {
                            (Some(request), Some(device)) => match open(request, device) {
                                Ok(mut reopened) => {
                                    reopened.retune(device.sample_rate, device.channels);
                                    let landed = seek_within(&mut reopened, seconds);
                                    playback = Some(reopened);
                                    landed
                                }
                                Err(error) => {
                                    tracing::debug!(%error, "reopening for a seek failed");
                                    false
                                }
                            },
                            _ => false,
                        },
                    };

                    if landed {
                        shared.ring.clear();
                        shared.frames.store(0, Ordering::Relaxed);
                        pending.clear();
                        sample_buffer = None;
                        seek_offset = seconds;
                        last_reported = f64::MIN;
                    }
                }
                Ok(Command::SetVolume(_)) => {
                    idle = false;
                }
                Err(TryRecvError::Empty) => break,
            }
        }

        if let (Some(active), Some(device)) = (playback.as_mut(), output.as_ref()) {
            if !pending.is_empty() {
                let written = shared.ring.push(&pending);
                pending.drain(..written);
                if written > 0 {
                    idle = false;
                }
            }

            if pending.is_empty() && !active.finished && shared.ring.free() > 4096 {
                match decode_block(
                    active,
                    device.channels,
                    &mut sample_buffer,
                    &mut mapped,
                    &mut resampled,
                ) {
                    Ok(Some(block)) => {
                        idle = false;
                        let written = shared.ring.push(block);
                        if written < block.len() {
                            pending.extend_from_slice(&block[written..]);
                        }
                    }
                    Ok(None) => {
                        active.finished = true;
                    }
                    Err(error) => {
                        active.finished = true;
                        let _ = events.send(Event::Failed(error));
                    }
                }
            }

            let position = seek_offset
                + shared.frames.load(Ordering::Relaxed) as f64 / device.sample_rate as f64;
            if (position - last_reported).abs() >= 0.2 {
                last_reported = position;
                let _ = events.send(Event::Position(position));
            }

            if active.finished && pending.is_empty() && shared.ring.is_empty() {
                shared.playing.store(false, Ordering::Relaxed);
                playback = None;
                let _ = events.send(Event::Finished);
            }
        }

        if idle {
            std::thread::sleep(IDLE_SLEEP);
        }
    }
}

fn seek_within(playback: &mut Playback, seconds: f64) -> bool {
    let target = SeekTo::Time {
        time: Time::from(seconds),
        track_id: Some(playback.track_id),
    };
    match playback.format.seek(SeekMode::Accurate, target) {
        Ok(_) => {
            playback.decoder.reset();
            playback.resampler.reset();
            playback.finished = false;
            true
        }
        Err(error) => {
            tracing::debug!(%error, "seek was refused by the reader");
            false
        }
    }
}

fn rebuild_if_needed(
    shared: &Arc<Shared>,
    device: Output,
    playback: &mut Playback,
) -> Result<Output, String> {
    let device = if device.sample_rate == playback.source_rate
        && device.channels == playback.source_channels
    {
        device
    } else {
        drop(device);
        build_output(
            shared,
            playback.source_rate,
            playback.source_channels as u16,
        )?
    };
    playback.retune(device.sample_rate, device.channels);
    Ok(device)
}

fn decode_block<'a>(
    playback: &mut Playback,
    output_channels: usize,
    sample_buffer: &mut Option<SampleBuffer<f32>>,
    mapped: &'a mut Vec<f32>,
    resampled: &'a mut Vec<f32>,
) -> Result<Option<&'a [f32]>, String> {
    let packet = loop {
        match playback.format.next_packet() {
            Ok(packet) if packet.track_id() == playback.track_id => break packet,
            Ok(_) => continue,
            Err(SymphoniaError::IoError(error))
                if error.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                return Ok(None);
            }
            Err(SymphoniaError::ResetRequired) => {
                playback.decoder.reset();
                continue;
            }
            Err(error) => return Err(format!("the stream ended unexpectedly: {error}")),
        }
    };

    let decoded = match playback.decoder.decode(&packet) {
        Ok(decoded) => decoded,
        Err(SymphoniaError::DecodeError(_)) => {
            mapped.clear();
            return Ok(Some(&mapped[..]));
        }
        Err(error) => return Err(format!("decoding failed: {error}")),
    };

    let spec = *decoded.spec();
    let buffer = sample_buffer
        .get_or_insert_with(|| SampleBuffer::<f32>::new(decoded.capacity() as u64, spec));
    buffer.copy_interleaved_ref(decoded);

    mapped.clear();
    map_channels(
        buffer.samples(),
        playback.source_channels,
        output_channels,
        mapped,
    );

    if playback.resampler.is_identity() {
        return Ok(Some(&mapped[..]));
    }

    resampled.clear();
    playback.resampler.process(mapped, resampled);
    Ok(Some(&resampled[..]))
}

fn codec_name(playback: &Playback) -> String {
    playback
        .format
        .tracks()
        .iter()
        .find(|track| track.id == playback.track_id)
        .and_then(|track| {
            symphonia::default::get_codecs()
                .get_codec(track.codec_params.codec)
                .map(|descriptor| descriptor.short_name.to_string())
        })
        .unwrap_or_else(|| "audio".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_shared_state_starts_silent_and_stopped() {
        let shared = Shared::new(1024);
        assert!(!shared.playing.load(Ordering::Relaxed));
        assert_eq!(shared.volume(), 1.0);
        assert_eq!(shared.gain(), 1.0);
        assert_eq!(shared.frames.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn the_ring_is_sized_for_several_seconds_of_stereo_audio() {
        let shared = Shared::new((48_000.0 * 2.0 * RING_SECONDS) as usize);
        assert!(shared.ring.capacity() >= 48_000 * 2 * 3);
        let bytes = (shared.ring.capacity() + 1) * std::mem::size_of::<f32>();
        assert!(bytes <= 4 * 1024 * 1024, "{bytes} bytes");
    }

    #[test]
    fn volume_is_clamped_into_range() {
        let (player, _events) = Player::spawn();
        player.set_volume(4.0);
        assert_eq!(player.volume(), 1.0);
        player.set_volume(-1.0);
        assert_eq!(player.volume(), 0.0);
        player.set_volume(0.5);
        assert_eq!(player.volume(), 0.5);
    }

    #[test]
    fn an_unplayable_url_reports_a_failure_rather_than_panicking() {
        let (player, events) = Player::spawn();
        player.play(PlayRequest {
            url: "http://127.0.0.1:1/nothing".into(),
            headers: Vec::new(),
            replay_gain: None,
            peak: None,
            decryption_key: None,
        });
        let mut saw_failure = false;
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while std::time::Instant::now() < deadline {
            match events.recv_timeout(Duration::from_millis(500)) {
                Ok(Event::Failed(_)) => {
                    saw_failure = true;
                    break;
                }
                Ok(_) => continue,
                Err(_) => continue,
            }
        }
        assert!(saw_failure, "the player should report the failure");
        assert!(!player.is_playing());
    }

    #[test]
    fn the_worker_shuts_down_cleanly_when_the_player_is_dropped() {
        let (player, events) = Player::spawn();
        drop(player);
        let mut disconnected = false;
        for _ in 0..40 {
            if matches!(
                events.recv_timeout(Duration::from_millis(50)),
                Err(mpsc::RecvTimeoutError::Disconnected)
            ) {
                disconnected = true;
                break;
            }
        }
        assert!(disconnected);
    }
}

#[cfg(test)]
mod decode_tests {
    use super::*;
    use crate::source::{MemoryRange, RangeSource};

    fn wav(sample_rate: u32, channels: u16, frames: &[[i16; 2]]) -> Vec<u8> {
        let bytes_per_frame = 2 * channels as u32;
        let data_len = frames.len() as u32 * bytes_per_frame;
        let mut out = Vec::with_capacity(44 + data_len as usize);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&sample_rate.to_le_bytes());
        out.extend_from_slice(&(sample_rate * bytes_per_frame).to_le_bytes());
        out.extend_from_slice(&(bytes_per_frame as u16).to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for frame in frames {
            for sample in frame.iter().take(channels as usize) {
                out.extend_from_slice(&sample.to_le_bytes());
            }
        }
        out
    }

    fn playback_of(bytes: Vec<u8>, output_rate: u32, output_channels: usize) -> Playback {
        let source = RangeSource::new(Box::new(MemoryRange::new(bytes, true)));
        let stream = MediaSourceStream::new(Box::new(source), MediaSourceStreamOptions::default());
        prepare(stream, output_rate, output_channels, Hint::new()).expect("stream is playable")
    }

    fn drain(playback: &mut Playback, output_channels: usize) -> Vec<f32> {
        let mut buffer = None;
        let mut mapped = Vec::new();
        let mut resampled = Vec::new();
        let mut collected = Vec::new();
        loop {
            match decode_block(
                playback,
                output_channels,
                &mut buffer,
                &mut mapped,
                &mut resampled,
            ) {
                Ok(Some(block)) => collected.extend_from_slice(block),
                Ok(None) => break,
                Err(error) => panic!("decode failed: {error}"),
            }
        }
        collected
    }

    #[test]
    fn a_real_stream_is_probed_into_a_playable_track() {
        let frames: Vec<[i16; 2]> = (0..4410).map(|i| [i as i16, -(i as i16)]).collect();
        let playback = playback_of(wav(44_100, 2, &frames), 44_100, 2);
        assert_eq!(playback.source_rate, 44_100);
        assert_eq!(playback.source_channels, 2);
        let duration = playback.duration.expect("duration is known");
        assert!((duration - 0.1).abs() < 0.01, "{duration}");
        assert_eq!(codec_name(&playback), "pcm_s16le");
    }

    #[test]
    fn decoding_reproduces_the_encoded_sample_values() {
        let frames = vec![[0, 0], [i16::MAX, i16::MIN], [16_384, -16_384]];
        let mut playback = playback_of(wav(44_100, 2, &frames), 44_100, 2);
        let samples = drain(&mut playback, 2);
        assert_eq!(samples.len(), 6);
        assert!(samples[0].abs() < 0.001);
        assert!((samples[2] - 1.0).abs() < 0.001, "{}", samples[2]);
        assert!((samples[3] + 1.0).abs() < 0.001, "{}", samples[3]);
        assert!((samples[4] - 0.5).abs() < 0.01, "{}", samples[4]);
    }

    #[test]
    fn every_frame_of_the_stream_reaches_the_output() {
        let frames: Vec<[i16; 2]> = (0..2205).map(|i| [(i % 3000) as i16, 0]).collect();
        let mut playback = playback_of(wav(44_100, 2, &frames), 44_100, 2);
        let samples = drain(&mut playback, 2);
        assert_eq!(samples.len(), 2205 * 2);
    }

    #[test]
    fn a_mono_source_is_widened_to_the_output_layout() {
        let frames: Vec<[i16; 2]> = (0..64).map(|i| [(i * 100) as i16, 0]).collect();
        let mut playback = playback_of(wav(48_000, 1, &frames), 48_000, 2);
        assert_eq!(playback.source_channels, 1);
        let samples = drain(&mut playback, 2);
        assert_eq!(samples.len(), 128);
        for pair in samples.chunks_exact(2) {
            assert_eq!(pair[0], pair[1]);
        }
    }

    #[test]
    fn a_downsampled_stream_produces_proportionally_fewer_samples() {
        let frames: Vec<[i16; 2]> = (0..960).map(|i| [(i % 500) as i16, 0]).collect();
        let mut playback = playback_of(wav(96_000, 2, &frames), 48_000, 2);
        assert!(!playback.resampler.is_identity());
        let samples = drain(&mut playback, 2);
        let produced_frames = samples.len() / 2;
        assert!(
            (produced_frames as i64 - 480).abs() <= 4,
            "{produced_frames} frames"
        );
    }

    #[test]
    fn retuning_after_the_device_is_chosen_removes_needless_resampling() {
        let frames: Vec<[i16; 2]> = (0..100).map(|i| [i as i16, i as i16]).collect();
        let mut playback = playback_of(wav(44_100, 2, &frames), 48_000, 2);
        assert!(
            !playback.resampler.is_identity(),
            "a stream opened against the wrong device rate starts out resampling"
        );

        playback.retune(44_100, 2);
        assert!(
            playback.resampler.is_identity(),
            "once the device runs at the source rate the audio must pass through untouched"
        );
    }

    #[test]
    fn a_matching_rate_needs_no_resampler() {
        let frames: Vec<[i16; 2]> = (0..100).map(|i| [i as i16, i as i16]).collect();
        let playback = playback_of(wav(48_000, 2, &frames), 48_000, 2);
        assert!(playback.resampler.is_identity());
    }

    #[test]
    fn a_stream_that_is_not_audio_is_rejected() {
        let source = RangeSource::new(Box::new(MemoryRange::new(
            b"not audio at all".to_vec(),
            true,
        )));
        let stream = MediaSourceStream::new(Box::new(source), MediaSourceStreamOptions::default());
        assert!(prepare(stream, 44_100, 2, Hint::new()).is_err());
    }

    #[test]
    fn seeking_inside_a_stream_lands_on_the_requested_position() {
        let frames: Vec<[i16; 2]> = (0..44_100).map(|i| [(i % 1000) as i16, 0]).collect();
        let mut playback = playback_of(wav(44_100, 2, &frames), 44_100, 2);
        let target = SeekTo::Time {
            time: Time::from(0.5),
            track_id: Some(playback.track_id),
        };
        let seeked = playback
            .format
            .seek(SeekMode::Accurate, target)
            .expect("seek succeeds");
        let drift = (seeked.actual_ts as i64 - 22_050).abs();
        assert!(drift <= 2_048, "seek landed {drift} frames from the target");
        let remaining = drain(&mut playback, 2).len() / 2;
        assert!(
            remaining > 21_000 && remaining < 23_000,
            "{remaining} frames left"
        );
    }
}
