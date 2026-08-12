use crate::convert::{LinearResampler, map_channels, replay_gain_scale};
use crate::ring::Ring;
use crate::source::{self, ByteRange, HttpRange, RangeSource};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{Arc, mpsc};
use std::time::Duration;
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Time;

const RING_SECONDS: f32 = 4.0;
const IDLE_SLEEP: Duration = Duration::from_millis(5);
const RESTING_SLEEP: Duration = Duration::from_millis(50);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub struct PlayRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub replay_gain: Option<f32>,
    pub peak: Option<f32>,
    pub decryption_key: Option<String>,
    pub expected_duration: Option<f64>,
}

const LENGTH_SLACK_SECONDS: f64 = 5.0;
const LENGTH_SLACK_FRACTION: f64 = 0.05;

pub fn is_the_wrong_recording(expected: Option<f64>, got: Option<f64>) -> bool {
    let (Some(expected), Some(got)) = (expected, got) else {
        return false;
    };
    if !(expected.is_finite() && got.is_finite()) || expected <= 0.0 || got <= 0.0 {
        return false;
    }
    let slack = LENGTH_SLACK_SECONDS.max(expected * LENGTH_SLACK_FRACTION);
    (expected - got).abs() > slack
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
        bits_per_sample: Option<u32>,
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
    settled: AtomicBool,
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
            settled: AtomicBool::new(false),
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
        let Some(worker) = self.worker.take() else {
            return;
        };
        let deadline = std::time::Instant::now() + SHUTDOWN_GRACE;
        while !self.shared.settled.load(Ordering::Acquire) {
            if std::time::Instant::now() >= deadline {
                return;
            }
            std::thread::sleep(IDLE_SLEEP);
        }
        let _ = worker.join();
    }
}

const FALLBACK_RATE: u32 = 48_000;
const FALLBACK_CHANNELS: usize = 2;

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
                && range.min_sample_rate() <= preferred_rate
                && range.max_sample_rate() >= preferred_rate
            {
                chosen = Some(range.with_sample_rate(preferred_rate));
                break;
            }
        }
        if chosen.is_none() {
            for range in &ranges {
                if range.sample_format() == cpal::SampleFormat::F32
                    && range.min_sample_rate() <= preferred_rate
                    && range.max_sample_rate() >= preferred_rate
                {
                    chosen = Some(range.with_sample_rate(preferred_rate));
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

    let sample_rate = config.sample_rate();
    let channels = config.channels() as usize;
    shared
        .output_channels
        .store(channels as u32, Ordering::Relaxed);

    let callback_shared = Arc::clone(shared);
    let stream = device
        .build_output_stream(
            config.config(),
            move |output: &mut [f32], _: &cpal::OutputCallbackInfo| {
                if !callback_shared.playing.load(Ordering::Relaxed) {
                    callback_shared.ring.settle();
                    output.fill(0.0);
                    return;
                }
                let filled = callback_shared.ring.pop(output);
                let level = callback_shared.volume() * callback_shared.gain();
                apply_level(&mut output[..filled], level);
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
    bits_per_sample: Option<u32>,
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    source_rate: u32,
    source_channels: usize,
    duration: Option<f64>,
    resampler: LinearResampler,
    finished: bool,
    failed: bool,
    unreadable_in_a_row: u32,
}

const GIVE_UP_AFTER_UNREADABLE: u32 = 64;

fn apply_level(output: &mut [f32], level: f32) {
    for sample in output.iter_mut() {
        *sample = (*sample * level).clamp(-1.0, 1.0);
    }
}

fn open(request: &PlayRequest) -> Result<Playback, String> {
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
        return Err(match backend.preview() {
            Some(detail) => format!("the source returned a message, not audio: {detail}"),
            None => format!("the source returned {kind}, not audio"),
        });
    }

    let mut source = RangeSource::new(Box::new(backend));
    source.prime().map_err(|error| error.to_string())?;

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
                flac_hint,
            );
        }
        None => MediaSourceStream::new(Box::new(source), MediaSourceStreamOptions::default()),
    };
    prepare(stream, hint)
}

fn length(seconds: Option<f64>) -> String {
    let Some(seconds) = seconds else {
        return "an unmeasured".into();
    };
    let whole = seconds.round().max(0.0) as u64;
    format!("{}:{:02}", whole / 60, whole % 60)
}

fn unreadable(error: SymphoniaError) -> String {
    if let SymphoniaError::IoError(io) = &error
        && io.kind() != std::io::ErrorKind::UnexpectedEof
    {
        return io.to_string();
    }
    tracing::debug!(%error, "the probe could not identify the stream");
    "this stream is not audio the client can read. the gateway may have returned an encrypted or \
     fragmented file"
        .to_string()
}

fn prepare(stream: MediaSourceStream<'static>, hint: Hint) -> Result<Playback, String> {
    let format = symphonia::default::get_probe()
        .probe(
            &hint,
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(unreadable)?;

    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| "the stream carries no audio track".to_string())?;

    let track_id = track.id;
    let frames = track.num_frames;
    let parameters = track
        .codec_params
        .as_ref()
        .and_then(|params| params.audio())
        .ok_or_else(|| "the stream carries no audio track".to_string())?
        .clone();

    let decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&parameters, &AudioDecoderOptions::default())
        .map_err(|error| format!("no decoder for this stream: {error}"))?;

    let source_rate = parameters.sample_rate.unwrap_or(FALLBACK_RATE);
    let source_channels = parameters
        .channels
        .as_ref()
        .map(|channels| channels.count())
        .unwrap_or(FALLBACK_CHANNELS);
    let duration = match (frames, parameters.sample_rate) {
        (Some(frames), Some(rate)) if rate > 0 => Some(frames as f64 / rate as f64),
        _ => None,
    };

    Ok(Playback {
        bits_per_sample: parameters.bits_per_sample,
        format,
        decoder,
        track_id,
        source_rate,
        source_channels,
        duration,
        resampler: LinearResampler::new(source_rate, source_rate, source_channels),
        finished: false,
        failed: false,
        unreadable_in_a_row: 0,
    })
}

fn run(commands: Receiver<Command>, events: Sender<Event>, shared: Arc<Shared>) {
    let settled = Arc::clone(&shared);
    serve(commands, events, shared);
    settled.settled.store(true, Ordering::Release);
}

fn serve(commands: Receiver<Command>, events: Sender<Event>, shared: Arc<Shared>) {
    let mut output: Option<Output> = None;
    let mut playback: Option<Playback> = None;
    let mut interleaved: Vec<f32> = Vec::new();
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
                    interleaved.clear();
                    seek_offset = 0.0;
                    last_reported = f64::MIN;
                    current = Some((*request).clone());
                    let _ = events.send(Event::Loading);

                    shared.gain.store(
                        replay_gain_scale(request.replay_gain, request.peak).to_bits(),
                        Ordering::Relaxed,
                    );

                    let mut opened = match open(&request) {
                        Ok(opened) => opened,
                        Err(error) => {
                            let _ = events.send(Event::Failed(error));
                            playback = None;
                            continue;
                        }
                    };

                    if is_the_wrong_recording(request.expected_duration, opened.duration) {
                        let _ = events.send(Event::Failed(format!(
                            "the source sent a {} recording where this track is {}, so it is not \
                             the same performance",
                            length(opened.duration),
                            length(request.expected_duration)
                        )));
                        playback = None;
                        continue;
                    }

                    let device = match output.take() {
                        Some(device) => Ok(device),
                        None => {
                            build_output(&shared, opened.source_rate, opened.source_channels as u16)
                        }
                    };
                    let device = match device
                        .and_then(|device| rebuild_if_needed(&shared, device, &mut opened))
                    {
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
                        bits_per_sample: opened.bits_per_sample,
                        codec: codec_name(&opened),
                    });
                    output = Some(device);
                    playback = Some(opened);
                    shared.playing.store(true, Ordering::Relaxed);
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
                            (Some(request), Some(device)) => match open(request) {
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
                        interleaved.clear();
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
                    &mut interleaved,
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
                        active.failed = true;
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

            if reached_the_end(active.finished, pending.is_empty(), shared.ring.is_empty()) {
                shared.playing.store(false, Ordering::Relaxed);
                let ended_cleanly = !active.failed;
                playback = None;
                if ended_cleanly {
                    let _ = events.send(Event::Finished);
                }
            }
        }

        if idle {
            std::thread::sleep(match playback.is_some() {
                true => IDLE_SLEEP,
                false => RESTING_SLEEP,
            });
        }
    }
}

fn reached_the_end(finished: bool, pending_empty: bool, ring_empty: bool) -> bool {
    finished && pending_empty && ring_empty
}

fn seek_within(playback: &mut Playback, seconds: f64) -> bool {
    let whole = seconds.max(0.0).trunc();
    let nanos = ((seconds.max(0.0) - whole) * 1e9) as u32;
    let Some(time) = Time::try_new(whole as i64, nanos.min(999_999_999)) else {
        return false;
    };
    let target = SeekTo::Time {
        time,
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
    interleaved: &mut Vec<f32>,
    mapped: &'a mut Vec<f32>,
    resampled: &'a mut Vec<f32>,
) -> Result<Option<&'a [f32]>, String> {
    let packet = loop {
        match playback.format.next_packet() {
            Ok(Some(packet)) if packet.track_id == playback.track_id => break packet,
            Ok(Some(_)) => continue,
            Ok(None) => return Ok(None),
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
        Ok(decoded) => {
            playback.unreadable_in_a_row = 0;
            decoded
        }
        Err(SymphoniaError::DecodeError(_)) => {
            playback.unreadable_in_a_row += 1;
            if playback.unreadable_in_a_row >= GIVE_UP_AFTER_UNREADABLE {
                return Err(
                    "this stream stopped decoding part way through, so what reaches the speakers \
                     would not be the whole track"
                        .into(),
                );
            }
            mapped.clear();
            return Ok(Some(&mapped[..]));
        }
        Err(error) => return Err(format!("decoding failed: {error}")),
    };

    decoded.copy_to_vec_interleaved(interleaved);

    mapped.clear();
    map_channels(
        interleaved,
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
        .and_then(|track| track.codec_params.as_ref()?.audio())
        .and_then(|params| {
            symphonia::default::get_codecs()
                .get_audio_decoder(params.codec)
                .map(|entry| entry.codec.info.short_name.to_string())
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
    fn a_boost_can_never_push_a_sample_past_full_scale() {
        let mut output = [0.9, -0.9, 0.2, -0.2];
        apply_level(&mut output, 2.0);
        assert_eq!(output, [1.0, -1.0, 0.4, -0.4]);
        assert!(
            output.iter().all(|sample| sample.abs() <= 1.0),
            "a sample outside full scale reaches the sound card as noise"
        );
    }

    #[test]
    fn ordinary_levels_pass_through_untouched() {
        let mut output = [0.5, -0.25];
        apply_level(&mut output, 1.0);
        assert_eq!(output, [0.5, -0.25]);
        apply_level(&mut output, 0.0);
        assert_eq!(output, [0.0, 0.0]);
    }

    #[test]
    fn the_ring_is_sized_for_several_seconds_of_stereo_audio() {
        let shared = Shared::new((48_000.0 * 2.0 * RING_SECONDS) as usize);
        assert!(shared.ring.capacity() >= 48_000 * 2 * 3);
        let bytes = (shared.ring.capacity() + 1) * std::mem::size_of::<f32>();
        assert!(bytes <= 4 * 1024 * 1024, "{bytes} bytes");
    }

    #[test]
    fn a_track_is_only_finished_once_everything_buffered_has_been_heard() {
        assert!(!reached_the_end(false, true, true));
        assert!(!reached_the_end(true, false, true));
        assert!(!reached_the_end(true, true, false));
        assert!(reached_the_end(true, true, true));
    }

    #[test]
    fn a_stream_that_failed_is_not_also_reported_as_finished() {
        let frames: Vec<[i16; 2]> = (0..64).map(|i| [i as i16, 0]).collect();
        let mut playback =
            decode_tests::playback_of(decode_tests::wav(44_100, 2, &frames), 44_100, 2);
        playback.finished = true;
        playback.failed = true;

        assert!(
            reached_the_end(playback.finished, true, true),
            "the track has stopped producing audio"
        );
        assert!(
            playback.failed,
            "a failed track must not be announced as a clean finish, or the queue advances"
        );
    }

    #[test]
    fn a_recording_of_the_right_length_is_accepted() {
        assert!(!is_the_wrong_recording(Some(185.0), Some(184.6)));
        assert!(!is_the_wrong_recording(Some(185.0), Some(189.0)));
        assert!(!is_the_wrong_recording(Some(600.0), Some(620.0)));
    }

    #[test]
    fn a_recording_of_quite_another_length_is_not_the_same_performance() {
        assert!(is_the_wrong_recording(Some(102.0), Some(163.0)));
        assert!(is_the_wrong_recording(Some(185.0), Some(240.0)));
    }

    #[test]
    fn a_length_nobody_measured_is_never_used_to_refuse_a_track() {
        assert!(!is_the_wrong_recording(None, Some(240.0)));
        assert!(!is_the_wrong_recording(Some(185.0), None));
        assert!(!is_the_wrong_recording(Some(0.0), Some(240.0)));
        assert!(!is_the_wrong_recording(Some(f64::NAN), Some(240.0)));
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
            expected_duration: None,
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

    pub(super) fn wav(sample_rate: u32, channels: u16, frames: &[[i16; 2]]) -> Vec<u8> {
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

    pub(super) fn playback_of(
        bytes: Vec<u8>,
        output_rate: u32,
        output_channels: usize,
    ) -> Playback {
        let source = RangeSource::new(Box::new(MemoryRange::new(bytes, true)));
        let stream = MediaSourceStream::new(Box::new(source), MediaSourceStreamOptions::default());
        let mut playback = prepare(stream, Hint::new()).expect("stream is playable");
        playback.retune(output_rate, output_channels);
        playback
    }

    fn drain(playback: &mut Playback, output_channels: usize) -> Vec<f32> {
        let mut buffer = Vec::new();
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
        assert!(prepare(stream, Hint::new()).is_err());
    }

    #[test]
    fn seeking_inside_a_stream_lands_on_the_requested_position() {
        let frames: Vec<[i16; 2]> = (0..44_100).map(|i| [(i % 1000) as i16, 0]).collect();
        let mut playback = playback_of(wav(44_100, 2, &frames), 44_100, 2);
        let target = SeekTo::Time {
            time: Time::try_new(0, 500_000_000).expect("half a second"),
            track_id: Some(playback.track_id),
        };
        let seeked = playback
            .format
            .seek(SeekMode::Accurate, target)
            .expect("seek succeeds");
        let drift = (seeked.actual_ts.get() - 22_050).abs();
        assert!(drift <= 2_048, "seek landed {drift} frames from the target");
        let remaining = drain(&mut playback, 2).len() / 2;
        assert!(
            remaining > 21_000 && remaining < 23_000,
            "{remaining} frames left"
        );
    }
}
