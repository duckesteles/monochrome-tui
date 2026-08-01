use monochrome_audio::{Event, PlayRequest, Player};
use std::time::{Duration, Instant};

fn wav(sample_rate: u32, channels: u16, frames: usize) -> Vec<u8> {
    let bytes_per_frame = 2 * channels as u32;
    let data_len = frames as u32 * bytes_per_frame;
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
    for frame in 0..frames {
        let phase = frame as f32 / sample_rate as f32 * 440.0 * std::f32::consts::TAU;
        let sample = (phase.sin() * 8000.0) as i16;
        for _ in 0..channels {
            out.extend_from_slice(&sample.to_le_bytes());
        }
    }
    out
}

struct Serving {
    url: String,
    _handle: std::thread::JoinHandle<()>,
}

fn serve(body: Vec<u8>) -> Serving {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("server binds");
    let port = server.server_addr().to_ip().expect("ip address").port();
    let handle = std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let response = tiny_http::Response::from_data(body.clone()).with_header(
                tiny_http::Header::from_bytes(&b"Accept-Ranges"[..], &b"bytes"[..])
                    .expect("header"),
            );
            let _ = request.respond(response);
        }
    });
    Serving {
        url: format!("http://127.0.0.1:{port}/audio.wav"),
        _handle: handle,
    }
}

fn collect(events: &std::sync::mpsc::Receiver<Event>, limit: Duration) -> Vec<Event> {
    let deadline = Instant::now() + limit;
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        match events.recv_timeout(Duration::from_millis(200)) {
            Ok(event) => {
                let finished = matches!(event, Event::Finished | Event::Failed(_));
                seen.push(event);
                if finished {
                    break;
                }
            }
            Err(_) => continue,
        }
    }
    seen
}

#[test]
fn a_track_served_over_http_plays_from_start_to_finish() {
    let serving = serve(wav(44_100, 2, 44_100 / 2));
    let (player, events) = Player::spawn();
    player.set_volume(0.0);
    player.play(PlayRequest {
        url: serving.url.clone(),
        headers: Vec::new(),
        replay_gain: None,
        peak: None,
        decryption_key: None,
    });

    let seen = collect(&events, Duration::from_secs(20));

    if let Some(Event::Failed(reason)) = seen.iter().find(|event| matches!(event, Event::Failed(_)))
    {
        assert!(
            reason.contains("audio") && reason.contains("device"),
            "playback failed for a reason other than a missing sound card: {reason}"
        );
        return;
    }

    let started = seen
        .iter()
        .find_map(|event| match event {
            Event::Started {
                duration,
                sample_rate,
                channels,
                bits_per_sample,
                codec,
            } => Some((
                *duration,
                *sample_rate,
                *channels,
                *bits_per_sample,
                codec.clone(),
            )),
            _ => None,
        })
        .expect("the player should report that the stream started");

    assert_eq!(started.1, 44_100);
    assert_eq!(started.2, 2);
    assert_eq!(started.3, Some(16));
    assert_eq!(started.4, "pcm_s16le");
    let duration = started.0.expect("the length should be known");
    assert!((duration - 0.5).abs() < 0.05, "reported {duration}s");

    assert!(
        seen.iter().any(|event| matches!(event, Event::Position(_))),
        "the player should report progress"
    );
    assert!(
        seen.iter().any(|event| matches!(event, Event::Finished)),
        "the player should reach the end of the track: {seen:?}"
    );
}

fn serve_json(status: u16, body: &'static str) -> Serving {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("server binds");
    let port = server.server_addr().to_ip().expect("ip address").port();
    let handle = std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let response = tiny_http::Response::from_string(body)
                .with_status_code(status)
                .with_header(
                    tiny_http::Header::from_bytes(
                        &b"Content-Type"[..],
                        &b"application/json; charset=utf-8"[..],
                    )
                    .expect("header"),
                );
            let _ = request.respond(response);
        }
    });
    Serving {
        url: format!("http://127.0.0.1:{port}/audio"),
        _handle: handle,
    }
}

fn failure_for(url: String) -> String {
    let (player, events) = Player::spawn();
    player.play(PlayRequest {
        url,
        headers: Vec::new(),
        replay_gain: None,
        peak: None,
        decryption_key: None,
    });
    collect(&events, Duration::from_secs(20))
        .into_iter()
        .find_map(|event| match event {
            Event::Failed(reason) => Some(reason),
            _ => None,
        })
        .expect("the player should report a failure")
}

fn wait_for_start(
    events: &std::sync::mpsc::Receiver<Event>,
    limit: Duration,
) -> Option<Result<(), String>> {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        match events.recv_timeout(Duration::from_millis(200)) {
            Ok(Event::Started { .. }) => return Some(Ok(())),
            Ok(Event::Failed(reason)) => return Some(Err(reason)),
            _ => continue,
        }
    }
    None
}

#[test]
fn switching_tracks_starts_the_new_one_promptly() {
    let first = serve(wav(44_100, 2, 44_100 * 30));
    let second = serve(wav(44_100, 2, 44_100 * 30));
    let (player, events) = Player::spawn();
    player.set_volume(0.0);

    let request = |url: String| PlayRequest {
        url,
        headers: Vec::new(),
        replay_gain: None,
        peak: None,
        decryption_key: None,
    };

    player.play(request(first.url.clone()));
    match wait_for_start(&events, Duration::from_secs(20)) {
        Some(Ok(())) => {}
        Some(Err(_)) => return,
        None => panic!("the first track should start"),
    }

    std::thread::sleep(Duration::from_millis(300));

    let switch = Instant::now();
    player.play(request(second.url.clone()));

    match wait_for_start(&events, Duration::from_secs(15)) {
        Some(Ok(())) => {}
        Some(Err(reason)) => panic!("switching failed: {reason}"),
        None => panic!("the second track should start"),
    }
    let taken = switch.elapsed();
    assert!(
        taken < Duration::from_secs(5),
        "switching took {taken:?}, which a listener would notice"
    );
}

#[test]
fn a_gateway_that_answers_with_json_instead_of_audio_reports_its_message() {
    let serving = serve_json(200, r#"{"detail":"Invalid Turnstile JWT."}"#);
    let reason = failure_for(serving.url.clone());
    assert!(
        reason.contains("Invalid Turnstile JWT."),
        "the gateway message should reach the user, got: {reason}"
    );
}

#[test]
fn a_gateway_error_status_carries_its_message_too() {
    let serving = serve_json(
        403,
        r#"{"error":"Forbidden: requests must come from an allowed site"}"#,
    );
    let reason = failure_for(serving.url.clone());
    assert!(
        reason.contains("allowed site"),
        "expected the gateway text, got: {reason}"
    );
}

#[test]
fn a_cd_rate_track_reaches_a_cd_rate_device_untouched() {
    let serving = serve(wav(44_100, 2, 44_100 / 2));
    let (player, events) = Player::spawn();
    player.set_volume(0.0);
    player.play(PlayRequest {
        url: serving.url.clone(),
        headers: Vec::new(),
        replay_gain: None,
        peak: None,
        decryption_key: None,
    });

    let seen = collect(&events, Duration::from_secs(20));
    let output = seen.iter().find_map(|event| match event {
        Event::Output {
            sample_rate,
            channels,
            resampling,
        } => Some((*sample_rate, *channels, *resampling)),
        _ => None,
    });

    let Some((sample_rate, channels, resampling)) = output else {
        return;
    };
    if sample_rate != 44_100 || channels != 2 {
        return;
    }
    assert!(
        !resampling,
        "a 44.1 kHz track was resampled onto a 44.1 kHz device, which loses the original samples"
    );
}

#[test]
fn a_source_that_answers_with_an_error_is_reported_not_ignored() {
    let (player, events) = Player::spawn();
    player.play(PlayRequest {
        url: "http://127.0.0.1:1/missing.wav".into(),
        headers: Vec::new(),
        replay_gain: None,
        peak: None,
        decryption_key: None,
    });
    let seen = collect(&events, Duration::from_secs(20));
    assert!(
        seen.iter().any(|event| matches!(event, Event::Failed(_))),
        "expected a failure, saw {seen:?}"
    );
}
