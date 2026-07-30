use crate::config::{Config, Paths};
use crate::secrets::{self, AMAZON_JWT, SESSION_TOKEN, Secrets};
use anyhow::{Context, Result};
use monochrome_api::error::ApiError;
use monochrome_api::{AuthClient, Catalog, StreamResolver};
use monochrome_audio::{PlayRequest, Player};
use std::time::Duration;

fn describe_address(seen: Option<String>, issued_for: &[String]) -> String {
    let Some(seen) = seen else {
        return "could not report the address it sees".into();
    };
    if issued_for.is_empty() {
        return "reachable".into();
    }
    if issued_for.iter().any(|address| address == &seen) {
        "sees the address the token was issued for".into()
    } else {
        "sees a different address than the token was issued for, verify again".into()
    }
}

pub async fn doctor(paths: Paths) -> Result<()> {
    let config = Config::load(&paths.config)?;
    let secrets = Secrets::new(paths.log_dir.join("credentials"));

    let catalog = Catalog::new(config.instances())?;
    match catalog.search_tracks("daft punk").await {
        Ok(tracks) => {
            let instance = catalog
                .active_instance()
                .map(|instance| instance.url.as_str())
                .unwrap_or("unknown");
            println!("catalog   ok, {} results from {instance}", tracks.len());
        }
        Err(error) => println!("catalog   FAILED: {}", secrets::redact(&error.to_string())),
    }

    let auth = AuthClient::new(&config.account.auth_url)?;
    match secrets.get(SESSION_TOKEN) {
        None => println!("session   not signed in"),
        Some(token) => match auth.me(&token).await {
            Ok(_) => {
                println!("session   ok, signed in");
                match auth.load_sync(&token).await {
                    Ok(document) => {
                        let library = monochrome_core::Library::new(document);
                        println!(
                            "library   {} tracks, {} albums, {} artists, {} playlists",
                            library.favorite_count(monochrome_core::FavoriteKind::Track),
                            library.favorite_count(monochrome_core::FavoriteKind::Album),
                            library.favorite_count(monochrome_core::FavoriteKind::Artist),
                            library.favorite_count(monochrome_core::FavoriteKind::Playlist),
                        );
                        println!("history   {} entries", library.history().len());
                        let readable = library.favorite_tracks().len();
                        let stored = library.favorite_count(monochrome_core::FavoriteKind::Track);
                        if readable != stored {
                            println!(
                                "          WARNING: {stored} saved tracks but only {readable} could be read"
                            );
                        }
                        for section in library.section_names() {
                            println!("          section {section}");
                        }
                    }
                    Err(error) => {
                        println!("library   FAILED: {}", secrets::redact(&error.to_string()))
                    }
                }
            }
            Err(error) => println!("session   FAILED: {}", secrets::redact(&error.to_string())),
        },
    }

    let resolver = StreamResolver::new(config.stream_config())?;
    if let Some(jwt) = secrets.get(AMAZON_JWT) {
        resolver.cache_jwt(jwt);
    }
    let seen = resolver.gateway_client_ip().await;
    let issued_for = secrets
        .get(AMAZON_JWT)
        .and_then(|token| monochrome_api::jwt::inspect(&token))
        .map(|claims| claims.addresses)
        .unwrap_or_default();
    println!("gateway   {}", describe_address(seen, &issued_for));

    if resolver.has_amazon_credential() {
        match resolver.validate_credential().await {
            Ok(()) => println!("amazon    credential present and accepted"),
            Err(ApiError::CredentialRejected) => {
                println!("amazon    credential present but REJECTED, verify again")
            }
            Err(ApiError::Status { code, message }) if code >= 500 => println!(
                "amazon    credential accepted, but the gateway itself failed: {code} {}",
                secrets::redact(&message)
            ),
            Err(error) => println!(
                "amazon    credential present, check failed: {}",
                secrets::redact(&error.to_string())
            ),
        }
    } else {
        println!("amazon    no credential, a browser check will be needed");
    }
    println!(
        "deezer    {}",
        if config.deezer.enabled {
            "enabled"
        } else {
            "disabled"
        }
    );

    Ok(())
}

pub async fn probe(paths: Paths, query: String) -> Result<()> {
    let config = Config::load(&paths.config)?;
    let secrets = Secrets::new(paths.log_dir.join("credentials"));
    let catalog = Catalog::new(config.instances())?;
    let resolver = StreamResolver::new(config.stream_config())?;
    if let Some(jwt) = secrets.get(AMAZON_JWT) {
        resolver.cache_jwt(jwt);
    }

    let track = catalog
        .search_tracks(&query)
        .await?
        .into_iter()
        .next()
        .context("nothing matched that search")?;
    println!(
        "track     {} \u{b7} {} ({})",
        track.title,
        track.artist_name(),
        track.id
    );
    println!("isrc      {}", track.isrc.as_deref().unwrap_or("none"));

    println!("credential {}", resolver.credential_kind());

    match resolver.amazon_lookup(&track, config.quality()).await {
        Ok(payload) => {
            println!("lookup    the gateway answered with:");
            for line in summarise_payload(&payload, "") {
                println!("            {line}");
            }
        }
        Err(error) => println!("lookup    FAILED: {}", secrets::redact(&error.to_string())),
    }

    if let Ok(payload) = resolver.amazon_lookup(&track, config.quality()).await
        && let Some(direct) = payload
            .get("stream_url")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    {
        println!("direct    fetching the cdn url the web client uses");
        let sample = tokio::task::spawn_blocking(move || {
            use monochrome_audio::source::ByteRange;
            let backend = monochrome_audio::source::HttpRange::open(&direct, &[])?;
            let content_type = backend.content_type();
            let length = backend.total_len();
            let mut reader = backend.open_at(0)?;
            let mut buffer = vec![0u8; 4096];
            let mut filled = 0;
            while filled < buffer.len() {
                match std::io::Read::read(&mut reader, &mut buffer[filled..]) {
                    Ok(0) => break,
                    Ok(read) => filled += read,
                    Err(error) => return Err(error),
                }
            }
            buffer.truncate(filled);
            Ok::<_, std::io::Error>((content_type, length, buffer))
        })
        .await?;

        match sample {
            Err(error) => println!("direct    FAILED: {}", secrets::redact(&error.to_string())),
            Ok((content_type, length, bytes)) => {
                println!(
                    "  type    {}",
                    content_type.as_deref().unwrap_or("not reported")
                );
                println!(
                    "  length  {}",
                    length
                        .map(|value| format!("{value} bytes"))
                        .unwrap_or_else(|| "not reported".into())
                );
                println!("  read    {} bytes", bytes.len());
                println!(
                    "  hex     {}",
                    monochrome_audio::probe::hex_preview(&bytes, 24)
                );
                println!(
                    "  ascii   {}",
                    monochrome_audio::probe::ascii_preview(&bytes, 24)
                );
                let boxes = monochrome_audio::probe::top_level_boxes(&bytes);
                if !boxes.is_empty() {
                    let listed: Vec<String> = boxes
                        .iter()
                        .map(|entry| format!("{}({})", entry.kind, entry.size))
                        .collect();
                    println!("  boxes   {}", listed.join(" "));
                }
                let markers = monochrome_audio::probe::encryption_markers(&bytes);
                println!(
                    "  crypto  {}",
                    if markers.is_empty() {
                        "no encryption boxes in the first 4 kB".to_string()
                    } else {
                        markers.join(" ")
                    }
                );
                println!("  verdict {}", monochrome_audio::probe::describe(&bytes));
            }
        }
    }

    let handle = match resolver.resolve(&track, config.quality()).await {
        Ok(handle) => handle,
        Err(error) => {
            println!("resolve   FAILED: {}", secrets::redact(&error.to_string()));
            return Ok(());
        }
    };
    println!("source    {}", handle.source.label());
    println!(
        "quality   {}",
        handle.quality.as_deref().unwrap_or("unreported")
    );
    println!(
        "url       {}",
        handle
            .url
            .split('/')
            .nth(2)
            .map(|host| format!("https://{host}/... (address hidden)"))
            .unwrap_or_else(|| "unreadable".into())
    );

    let url = handle.url.clone();
    let headers = handle.headers.clone();
    let sample = tokio::task::spawn_blocking(move || {
        use monochrome_audio::source::ByteRange;
        let backend = monochrome_audio::source::HttpRange::open(&url, &headers)?;
        let content_type = backend.content_type();
        let length = backend.total_len();
        let ranges = backend.supports_ranges();
        let mut reader = backend.open_at(0)?;
        let mut buffer = vec![0u8; 4096];
        let mut filled = 0;
        while filled < buffer.len() {
            match std::io::Read::read(&mut reader, &mut buffer[filled..]) {
                Ok(0) => break,
                Ok(read) => filled += read,
                Err(error) => return Err(error),
            }
        }
        buffer.truncate(filled);
        Ok::<_, std::io::Error>((content_type, length, ranges, buffer))
    })
    .await?;

    match sample {
        Err(error) => println!("fetch     FAILED: {}", secrets::redact(&error.to_string())),
        Ok((content_type, length, ranges, bytes)) => {
            println!(
                "type      {}",
                content_type.as_deref().unwrap_or("not reported")
            );
            println!(
                "length    {}",
                length
                    .map(|value| format!("{value} bytes"))
                    .unwrap_or_else(|| "not reported".into())
            );
            println!("ranges    {}", if ranges { "yes" } else { "no" });
            println!("read      {} bytes", bytes.len());
            println!(
                "hex       {}",
                monochrome_audio::probe::hex_preview(&bytes, 32)
            );
            println!(
                "ascii     {}",
                monochrome_audio::probe::ascii_preview(&bytes, 32)
            );
            let boxes = monochrome_audio::probe::top_level_boxes(&bytes);
            if !boxes.is_empty() {
                let listed: Vec<String> = boxes
                    .iter()
                    .map(|entry| format!("{}({})", entry.kind, entry.size))
                    .collect();
                println!("boxes     {}", listed.join(" "));
            }
            println!("verdict   {}", monochrome_audio::probe::describe(&bytes));
        }
    }

    Ok(())
}

pub(crate) fn summarise_payload(value: &serde_json::Value, prefix: &str) -> Vec<String> {
    let mut lines = Vec::new();
    match value {
        serde_json::Value::Object(fields) => {
            for (key, entry) in fields {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                match entry {
                    serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
                        lines.extend(summarise_payload(entry, &path))
                    }
                    other => lines.push(format!("{path} = {}", summarise_value(key, other))),
                }
            }
        }
        serde_json::Value::Array(items) => {
            lines.push(format!("{prefix} = [{} items]", items.len()));
        }
        other => lines.push(format!("{prefix} = {other}")),
    }
    lines
}

pub(crate) fn summarise_value(key: &str, value: &serde_json::Value) -> String {
    let lowered = key.to_ascii_lowercase();
    let text = match value {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    if lowered.contains("key") && text.len() > 8 {
        return format!("<{} characters, hidden>", text.len());
    }
    if lowered.contains("url") && text.starts_with("https://") {
        return match text.split('/').nth(2) {
            Some(host) => format!("https://{host}/... <{} characters, hidden>", text.len()),
            None => format!("<{} characters, hidden>", text.len()),
        };
    }
    if text.len() > 96 {
        return format!("{}... ({} characters)", &text[..96], text.len());
    }
    text
}

pub async fn play_once(paths: Paths, query: String) -> Result<()> {
    if std::env::var_os("RUST_LOG").is_some() {
        let _ = tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .try_init();
    }

    let config = Config::load(&paths.config)?;
    let secrets = Secrets::new(paths.log_dir.join("credentials"));
    let catalog = Catalog::new(config.instances())?;
    let resolver = StreamResolver::new(config.stream_config())?;
    if let Some(jwt) = secrets.get(AMAZON_JWT) {
        resolver.cache_jwt(jwt);
    }

    let (player, events) = Player::spawn();
    player.set_volume(0.0);

    let wanted: Vec<&str> = query
        .split(';')
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .collect();
    let switching = wanted.len() > 1;

    for (index, wanted) in wanted.iter().enumerate() {
        let track = catalog
            .search_tracks(wanted)
            .await?
            .into_iter()
            .next()
            .context("nothing matched that search")?;
        println!("track     {} \u{b7} {}", track.title, track.artist_name());

        let handle = resolver.resolve(&track, config.quality()).await?;
        if index == 0 {
            println!("source    {}", handle.source.label());
            println!(
                "encrypted {}",
                if handle.decryption_key.is_some() {
                    "yes, decrypting locally"
                } else {
                    "no"
                }
            );
        }

        let asked_at = std::time::Instant::now();
        player.play(PlayRequest {
            url: handle.url,
            headers: handle.headers,
            replay_gain: track.replay_gain,
            peak: track.peak,
            decryption_key: handle.decryption_key,
        });

        let mut started = false;
        let deadline = std::time::Instant::now() + Duration::from_secs(40);
        let mut furthest = 0.0f64;
        let mut seek_sent = false;
        let mut seek_back_worked = false;

        while std::time::Instant::now() < deadline {
            match events.recv_timeout(Duration::from_millis(200)) {
                Ok(monochrome_audio::Event::Started {
                    duration,
                    sample_rate,
                    channels,
                    codec,
                }) => {
                    started = true;
                    println!(
                        "{:<9} {:.2} s to first audio",
                        if index == 0 { "latency" } else { "switch" },
                        asked_at.elapsed().as_secs_f64()
                    );
                    println!(
                        "started   {codec} {sample_rate} Hz, {channels} channels, {}",
                        duration
                            .map(|value| format!("{value:.1} s"))
                            .unwrap_or_else(|| "length unknown".into())
                    );
                    if switching {
                        break;
                    }
                }
                Ok(monochrome_audio::Event::Output {
                    sample_rate,
                    channels,
                    resampling,
                }) => println!(
                    "output    {sample_rate} Hz, {channels} channels, {}",
                    if resampling {
                        "RESAMPLED"
                    } else {
                        "bit exact, no resampling"
                    }
                ),
                Ok(monochrome_audio::Event::Position(position)) => {
                    if seek_sent && position < 3.0 {
                        seek_back_worked = true;
                        break;
                    }
                    furthest = furthest.max(position);
                    if furthest > 6.0 && !seek_sent {
                        seek_sent = true;
                        player.seek_to(1.0);
                    }
                }
                Ok(monochrome_audio::Event::Failed(reason)) => {
                    println!("FAILED    {}", secrets::redact(&reason));
                    return Ok(());
                }
                Ok(monochrome_audio::Event::Finished) => break,
                Ok(_) => {}
                Err(_) => {}
            }
        }

        if !started {
            println!("FAILED    nothing started within the time allowed");
            return Ok(());
        }
        if seek_sent {
            println!(
                "seek back {}",
                if seek_back_worked {
                    "works"
                } else {
                    "DID NOT WORK"
                }
            );
        }
        if switching && index + 1 < wanted_len(&query) {
            std::thread::sleep(Duration::from_millis(600));
        }
    }

    Ok(())
}

pub(crate) fn wanted_len(query: &str) -> usize {
    query.split(';').filter(|q| !q.trim().is_empty()).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_decryption_key_is_never_printed() {
        let shown = summarise_value("decryption_key", &json!("00112233445566778899aabbccddeeff"));
        assert!(!shown.contains("00112233"));
        assert!(shown.contains("hidden"));
    }

    #[test]
    fn a_signed_address_is_reduced_to_its_host() {
        let address = format!("https://cdn.example.net/{}?Signature=abc", "x".repeat(120));
        let shown = summarise_value("stream_url", &json!(address));
        assert!(shown.starts_with("https://cdn.example.net/"));
        assert!(!shown.contains("Signature"));
        assert!(shown.contains("hidden"));
    }

    #[test]
    fn an_ordinary_field_is_shown_as_it_is() {
        assert_eq!(
            summarise_value("album_name", &json!("Discovery")),
            "Discovery"
        );
        assert_eq!(summarise_value("expires_in", &json!(157)), "157");
        assert_eq!(summarise_value("asin", &json!("B0064UPUDC")), "B0064UPUDC");
    }

    #[test]
    fn a_long_ordinary_value_is_truncated_rather_than_dumped() {
        let shown = summarise_value("description", &json!("a".repeat(500)));
        assert!(shown.len() < 200);
        assert!(shown.contains("500 characters"));
    }

    #[test]
    fn a_short_key_field_is_left_alone_because_it_is_not_a_secret() {
        assert_eq!(summarise_value("key", &json!("G")), "G");
        assert_eq!(summarise_value("keyScale", &json!("MAJOR")), "MAJOR");
    }

    #[test]
    fn nested_payloads_are_flattened_with_their_paths() {
        let payload = json!({
            "asin": "B0064UPUDC",
            "match": { "confidence": "high", "score": 95.9 },
            "available_qualities": ["HD", "UHD"],
        });
        let mut lines = summarise_payload(&payload, "");
        lines.sort();
        assert!(lines.contains(&"asin = B0064UPUDC".to_string()));
        assert!(lines.contains(&"match.confidence = high".to_string()));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("available_qualities"))
        );
    }

    #[test]
    fn a_key_nested_inside_the_payload_is_still_hidden() {
        let payload = json!({ "stream": { "decryption_key": "00112233445566778899aabbccddeeff" } });
        let lines = summarise_payload(&payload, "");
        assert!(lines.iter().all(|line| !line.contains("00112233")));
        assert!(lines.iter().any(|line| line.contains("hidden")));
    }

    #[test]
    fn several_queries_are_counted_for_a_switching_run() {
        assert_eq!(wanted_len("one"), 1);
        assert_eq!(wanted_len("one; two; three"), 3);
        assert_eq!(wanted_len("one;;two"), 2);
        assert_eq!(wanted_len("  "), 0);
    }

    #[test]
    fn the_gateways_view_is_compared_rather_than_printed() {
        let issued = vec!["203.0.113.7".to_string()];
        assert_eq!(
            describe_address(Some("203.0.113.7".into()), &issued),
            "sees the address the token was issued for"
        );
        assert_eq!(
            describe_address(Some("198.51.100.4".into()), &issued),
            "sees a different address than the token was issued for, verify again"
        );
    }

    #[test]
    fn no_address_ever_reaches_the_report() {
        let issued = vec!["203.0.113.7".to_string()];
        for seen in [
            Some("203.0.113.7".to_string()),
            Some("198.51.100.4".to_string()),
            None,
        ] {
            let line = describe_address(seen, &issued);
            assert!(!line.contains("203.0.113"), "leaked: {line}");
            assert!(!line.contains("198.51.100"), "leaked: {line}");
        }
    }

    #[test]
    fn without_a_token_there_is_nothing_to_compare_against() {
        assert_eq!(
            describe_address(Some("203.0.113.7".into()), &[]),
            "reachable"
        );
        assert_eq!(
            describe_address(None, &[]),
            "could not report the address it sees"
        );
    }
}
