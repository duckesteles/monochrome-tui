use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{Event, EventStream, KeyEventKind};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures::StreamExt;
use monochrome_api::error::ApiError;
use monochrome_api::jwt;
use monochrome_api::{AuthClient, Catalog, StreamResolver};
use monochrome_audio::{PlayRequest, Player};
use monochrome_core::model::Track;
use monochrome_tui::app::{App, ArtistPage, Effect, Focus, Message};
use monochrome_tui::config::{Config, Paths};
use monochrome_tui::dispatch;
use monochrome_tui::secrets::{self, AMAZON_JWT, SESSION_TOKEN, Secrets};
use monochrome_tui::sync::SyncScheduler;
use monochrome_tui::theme::Theme;
use monochrome_tui::views;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::widgets::ListState;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

const STATUS_LIFETIME: Duration = Duration::from_secs(5);
const SYNC_DEBOUNCE: Duration = Duration::from_secs(5);

#[derive(Parser, Debug)]
#[command(name = "monochrome", about = "A terminal client for monochrome.tf")]
struct Args {
    #[arg(long, help = "Write a log file to the state directory")]
    verbose: bool,
    #[arg(long, help = "Print the resolved file locations and exit")]
    paths: bool,
    #[arg(long, help = "Check the services and your account, then exit")]
    doctor: bool,
    #[arg(
        long,
        value_name = "QUERY",
        help = "Resolve a track and report what the gateway actually sends"
    )]
    probe: Option<String>,
    #[arg(
        long,
        value_name = "QUERY",
        help = "Play the first match for a few seconds, then exit"
    )]
    play: Option<String>,
}

struct Services {
    catalog: Catalog,
    auth: AuthClient,
    resolver: StreamResolver,
    secrets: Secrets,
    quality: monochrome_core::model::Quality,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let paths = Paths::resolve()?;

    if args.paths {
        println!("config    {}", paths.config.display());
        println!("snapshot  {}", paths.snapshot.display());
        println!("state     {}", paths.log_dir.display());
        return Ok(());
    }

    let _guard = args
        .verbose
        .then(|| setup_logging(&paths.log_dir))
        .transpose()?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;

    if args.doctor {
        return runtime.block_on(doctor(paths));
    }
    if let Some(query) = args.probe {
        return runtime.block_on(probe(paths, query));
    }
    if let Some(query) = args.play {
        return runtime.block_on(play_once(paths, query));
    }
    runtime.block_on(run(paths))
}

fn setup_logging(dir: &std::path::Path) -> Result<tracing_appender::non_blocking::WorkerGuard> {
    std::fs::create_dir_all(dir)?;
    let appender = tracing_appender::rolling::never(dir, "log");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "monochrome=debug,info".into()),
        )
        .init();
    Ok(guard)
}

async fn doctor(paths: Paths) -> Result<()> {
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
            Ok(user) => {
                println!("session   ok, signed in as {}", user.display_name());
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
    match resolver.gateway_client_ip().await {
        Some(address) => println!("gateway   sees this client as {address}"),
        None => println!("gateway   could not report the address it sees"),
    }

    if resolver.has_amazon_credential() {
        match resolver.validate_credential().await {
            Ok(()) => println!("amazon    credential present and accepted"),
            Err(ApiError::CredentialRejected) => {
                println!("amazon    credential present but REJECTED, paste a fresh token")
            }
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

async fn probe(paths: Paths, query: String) -> Result<()> {
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

fn summarise_payload(value: &serde_json::Value, prefix: &str) -> Vec<String> {
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

fn summarise_value(key: &str, value: &serde_json::Value) -> String {
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

async fn play_once(paths: Paths, query: String) -> Result<()> {
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
    println!("track     {} \u{b7} {}", track.title, track.artist_name());

    let handle = resolver.resolve(&track, config.quality()).await?;
    println!("source    {}", handle.source.label());
    println!(
        "encrypted {}",
        if handle.decryption_key.is_some() {
            "yes, decrypting locally"
        } else {
            "no"
        }
    );

    let (player, events) = Player::spawn();
    player.set_volume(0.0);
    player.play(PlayRequest {
        url: handle.url,
        headers: handle.headers,
        replay_gain: track.replay_gain,
        peak: track.peak,
        decryption_key: handle.decryption_key,
    });

    let deadline = std::time::Instant::now() + Duration::from_secs(40);
    let mut furthest = 0.0f64;
    let mut seek_sent = false;
    let mut seek_back_worked = false;

    while std::time::Instant::now() < deadline {
        match events.recv_timeout(Duration::from_millis(500)) {
            Ok(monochrome_audio::Event::Started {
                duration,
                sample_rate,
                channels,
                codec,
            }) => {
                println!(
                    "started   {codec} {sample_rate} Hz, {channels} channels, {}",
                    duration
                        .map(|value| format!("{value:.1} s"))
                        .unwrap_or_else(|| "length unknown".into())
                );
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

    if furthest > 0.0 {
        println!("played    {furthest:.1} seconds of audio");
    } else {
        println!("played    nothing came out");
    }
    Ok(())
}

async fn run(paths: Paths) -> Result<()> {
    let mut config = Config::load(&paths.config)?;
    if !paths.config.exists() {
        config.save(&paths.config).ok();
    }

    let secrets = Secrets::new(paths.log_dir.join("credentials"));
    let resolver = StreamResolver::new(config.stream_config())?;
    if let Some(jwt) = secrets.get(AMAZON_JWT) {
        resolver.cache_jwt(jwt);
    }

    let services = Arc::new(Services {
        catalog: Catalog::new(config.instances())?,
        auth: AuthClient::new(&config.account.auth_url)?,
        resolver,
        secrets,
        quality: config.quality(),
    });

    let (player, audio_events) = Player::spawn();
    let player = Arc::new(player);
    player.set_volume(config.volume());

    let (messages, mut inbox) = unbounded_channel::<Message>();
    forward_audio_events(audio_events, messages.clone());

    let mut app = App::new(config.quality(), config.volume());
    app.library = monochrome_core::Library::new(load_snapshot(&paths));

    match services.secrets.get(SESSION_TOKEN) {
        Some(token) => restore_session(services.clone(), token, messages.clone()),
        None => app.focus = Focus::Login,
    }

    let mut terminal = enter_terminal()?;
    let theme = Theme::new(&config.ui.accent);
    let mut list = ListState::default();
    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(500));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut status_since = Instant::now();
    let mut bridge_offered = false;
    let mut scheduler = SyncScheduler::new(SYNC_DEBOUNCE);
    let mut redraw = true;

    loop {
        if redraw {
            terminal.draw(|frame| views::render(frame, &app, &theme, &mut list))?;
            redraw = false;
        }

        let effects = tokio::select! {
            event = events.next() => match event {
                Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                    redraw = true;
                    {
                        status_since = Instant::now();
                        dispatch::on_key(&mut app, key)
                    }
                }
                Some(Ok(Event::Resize(_, _))) => {
                    redraw = true;
                    Vec::new()
                }
                Some(Ok(_)) => Vec::new(),
                Some(Err(_)) | None => break,
            },
            message = inbox.recv() => match message {
                Some(message) => {
                    redraw = true;
                    if matches!(message, Message::Notice(_) | Message::Failure(_) | Message::SignInFailed(_)) {
                        status_since = Instant::now();
                    }
                    match &message {
                        Message::NeedsVerification(_) => bridge_offered = true,
                        Message::Verified => bridge_offered = false,
                        _ => {}
                    }
                    let arrived_from_server = matches!(message, Message::Sync(_));
                    let effects = app.apply(message);
                    if arrived_from_server {
                        save_snapshot(&app, &paths);
                    }
                    effects
                }
                None => break,
            },
            _ = ticker.tick() => {
                if app.status.is_some() && status_since.elapsed() > STATUS_LIFETIME {
                    app.status = None;
                    redraw = true;
                }
                if scheduler.take_if_due(Instant::now()) {
                    push_sync(&mut app, &services, &messages, &paths);
                    redraw = true;
                }
                Vec::new()
            }
        };

        for effect in effects {
            match effect {
                Effect::Quit => app.quit = true,
                Effect::PushSync => scheduler.request(Instant::now()),
                Effect::OpenBrowser => match app.verification_url.clone() {
                    Some(url) if open::that_detached(&url).is_ok() => {
                        app.status = Some("waiting for the browser check".into());
                    }
                    Some(_) => {
                        app.status = Some("could not open a browser, copy the address above".into())
                    }
                    None => {}
                },
                other => perform(other, &services, &player, &messages, !bridge_offered),
            }
        }

        if app.quit {
            break;
        }
    }

    if scheduler.take_now() {
        flush_on_exit(&mut app, &services, &paths).await;
    } else {
        save_snapshot(&app, &paths);
    }
    if (app.volume - config.volume()).abs() > f32::EPSILON {
        config.playback.volume = app.volume;
        let _ = config.save(&paths.config);
    }
    leave_terminal(terminal)?;
    Ok(())
}

fn perform(
    effect: Effect,
    services: &Arc<Services>,
    player: &Arc<Player>,
    messages: &UnboundedSender<Message>,
    allow_bridge: bool,
) {
    match effect {
        Effect::Quit | Effect::PushSync => {}
        Effect::Search(query) => {
            let services = services.clone();
            let messages = messages.clone();
            tokio::spawn(async move {
                let results = services.catalog.search(&query).await;
                let _ = messages.send(Message::SearchResults(Box::new(results)));
            });
        }
        Effect::LoadAlbum(id) => {
            let services = services.clone();
            let messages = messages.clone();
            tokio::spawn(async move {
                match services.catalog.album(id).await {
                    Ok(album) => {
                        let _ = messages.send(Message::Album(Box::new(album)));
                    }
                    Err(error) => report(&messages, error),
                }
            });
        }
        Effect::LoadArtist(id) => {
            let services = services.clone();
            let messages = messages.clone();
            tokio::spawn(async move {
                let (artist, albums, tracks) = tokio::join!(
                    services.catalog.artist(id),
                    services.catalog.artist_albums(id),
                    services.catalog.artist_top_tracks(id),
                );
                match artist {
                    Ok(artist) => {
                        let _ = messages.send(Message::Artist(Box::new(ArtistPage {
                            artist,
                            albums: albums.unwrap_or_default(),
                            top_tracks: tracks.unwrap_or_default(),
                        })));
                    }
                    Err(error) => report(&messages, error),
                }
            });
        }
        Effect::LoadPlaylist(uuid) => {
            let services = services.clone();
            let messages = messages.clone();
            tokio::spawn(async move {
                match services.catalog.playlist(&uuid).await {
                    Ok((playlist, tracks)) => {
                        let _ = messages.send(Message::Playlist(Box::new(playlist), tracks));
                    }
                    Err(error) => report(&messages, error),
                }
            });
        }
        Effect::Play(track) => start_playback(*track, services, player, messages, allow_bridge),
        Effect::Pause => player.pause(),
        Effect::Resume => player.resume(),
        Effect::Stop => player.stop(),
        Effect::Seek(position) => player.seek_to(position),
        Effect::Volume(volume) => player.set_volume(volume),
        Effect::SignIn {
            email,
            mut password,
        } => {
            let services = services.clone();
            let messages = messages.clone();
            tokio::spawn(async move {
                let outcome = services.auth.sign_in(&email, &password).await;
                password.clear();
                match outcome {
                    Ok((token, user)) => {
                        let _ = services.secrets.set(SESSION_TOKEN, &token);
                        let _ = messages.send(Message::SignedIn(Box::new(user)));
                        load_sync(&services, &token, &messages).await;
                    }
                    Err(error) => {
                        let _ = messages
                            .send(Message::SignInFailed(secrets::redact(&error.to_string())));
                    }
                }
            });
        }
        Effect::UseToken(token) => {
            let services = services.clone();
            let messages = messages.clone();
            tokio::spawn(async move {
                services.resolver.cache_jwt(token.clone());
                match services.resolver.validate_credential().await {
                    Ok(()) => {
                        let _ = services.secrets.set(AMAZON_JWT, &token);
                        let _ = messages.send(Message::Verified);
                    }
                    Err(ApiError::CredentialRejected) => {
                        services.secrets.clear(AMAZON_JWT);
                        let reason = explain_rejection(&token, &services).await;
                        let _ = messages.send(Message::VerificationFailed(reason));
                    }
                    Err(error) => {
                        let _ = messages.send(Message::VerificationFailed(secrets::redact(
                            &error.to_string(),
                        )));
                    }
                }
            });
        }
        Effect::OpenBrowser => {}

        Effect::SignOut => {
            let services = services.clone();
            let messages = messages.clone();
            let token = services.secrets.get(SESSION_TOKEN);
            services.secrets.clear(SESSION_TOKEN);
            tokio::spawn(async move {
                if let Some(token) = token {
                    let _ = services.auth.sign_out(&token).await;
                }
                let _ = messages.send(Message::SignedOut);
            });
        }
    }
}

fn start_playback(
    track: Track,
    services: &Arc<Services>,
    player: &Arc<Player>,
    messages: &UnboundedSender<Message>,
    allow_bridge: bool,
) {
    let services = services.clone();
    let player = player.clone();
    let messages = messages.clone();
    tokio::spawn(async move {
        match services.resolver.resolve(&track, services.quality).await {
            Ok(handle) => {
                let _ = messages.send(Message::StreamReady {
                    source: handle.source.label().to_string(),
                });
                player.play(PlayRequest {
                    url: handle.url,
                    headers: handle.headers,
                    replay_gain: track.replay_gain,
                    peak: track.peak,
                    decryption_key: handle.decryption_key,
                });
            }
            Err(ApiError::CredentialRejected) => {
                let _ = messages.send(Message::VerificationFailed(
                    "the stored amazon token expired. copy a fresh one from monochrome.tf and \
                     paste it below."
                        .into(),
                ));
            }
            Err(ApiError::TurnstileRequired) if !allow_bridge => {
                let _ = messages.send(Message::VerificationFailed(
                    "the amazon gateway still refuses the token. paste a fresh one from \
                     monochrome.tf, or set a bypass token in the config."
                        .into(),
                ));
            }
            Err(ApiError::TurnstileRequired) => {
                match services.resolver.start_verification().await {
                    Ok(bridge) => {
                        let _ = messages.send(Message::NeedsVerification(bridge.url()));
                        match bridge.wait_for_token().await {
                            Ok(token) => {
                                match services.resolver.finish_verification(&token).await {
                                    Ok(()) => {
                                        persist_amazon_jwt(&services);
                                        let _ = messages.send(Message::Verified);
                                    }
                                    Err(error) => {
                                        let _ = messages.send(Message::VerificationFailed(
                                            secrets::redact(&error.to_string()),
                                        ));
                                    }
                                }
                            }
                            Err(error) => {
                                let _ = messages.send(Message::VerificationFailed(
                                    secrets::redact(&error.to_string()),
                                ));
                            }
                        }
                    }
                    Err(error) => report(&messages, error),
                }
            }
            Err(error) => {
                let _ = messages.send(Message::PlaybackFailed(secrets::redact(&error.to_string())));
            }
        }
    });
}

async fn explain_rejection(token: &str, services: &Arc<Services>) -> String {
    let Some(claims) = jwt::inspect(token) else {
        return "that did not look like a token. copy the whole value the console prints, \
                without quotes."
            .into();
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();

    if claims.is_expired(now) == Some(true) {
        let age = claims.expires_at.map(|exp| now.saturating_sub(exp) / 60);
        return match age {
            Some(minutes) => format!(
                "that token expired {minutes} minutes ago. these last about an hour, so copy a \
                 fresh one from monochrome.tf."
            ),
            None => "that token has expired. copy a fresh one from monochrome.tf.".into(),
        };
    }

    if let Some(issued_for) = claims.addresses.first() {
        let seen = services.resolver.gateway_client_ip().await;
        return match seen {
            Some(seen) if seen != *issued_for => format!(
                "that token was issued for {issued_for} but the gateway sees this client as \
                 {seen}. your browser and this client are reaching the internet by different \
                 routes, so the token cannot be shared. send both through the same route, or \
                 set an amazon bypass token in the config."
            ),
            _ => "the gateway refused that token even though it looks valid and unexpired.".into(),
        };
    }

    "the gateway refused that token. it may already have been used, or it is bound to the \
     browser that fetched it."
        .into()
}

fn persist_amazon_jwt(services: &Arc<Services>) {
    if let Some(jwt) = services.resolver.cached_jwt() {
        let _ = services.secrets.set(AMAZON_JWT, &jwt);
    }
}

fn push_sync(
    app: &mut App,
    services: &Arc<Services>,
    messages: &UnboundedSender<Message>,
    paths: &Paths,
) {
    save_snapshot(app, paths);
    let changes = app.library.take_dirty();
    if changes.is_empty() {
        return;
    }
    let Some(token) = services.secrets.get(SESSION_TOKEN) else {
        return;
    };
    app.syncing = true;
    let services = services.clone();
    let messages = messages.clone();
    tokio::spawn(async move {
        match services.auth.push_sync(&token, &changes).await {
            Ok(document) => {
                let _ = messages.send(Message::Sync(Box::new(document)));
            }
            Err(ApiError::Unauthorized) => {
                services.secrets.clear(SESSION_TOKEN);
                let _ = messages.send(Message::SignedOut);
            }
            Err(error) => report(&messages, error),
        }
    });
}

fn restore_session(services: Arc<Services>, token: String, messages: UnboundedSender<Message>) {
    tokio::spawn(async move {
        match services.auth.me(&token).await {
            Ok(user) => {
                let _ = messages.send(Message::SignedIn(Box::new(user)));
                load_sync(&services, &token, &messages).await;
            }
            Err(ApiError::Unauthorized) => {
                services.secrets.clear(SESSION_TOKEN);
                let _ = messages.send(Message::SignedOut);
            }
            Err(error) => report(&messages, error),
        }
    });
}

async fn load_sync(services: &Arc<Services>, token: &str, messages: &UnboundedSender<Message>) {
    match services.auth.load_sync(token).await {
        Ok(document) => {
            let _ = messages.send(Message::Sync(Box::new(document)));
        }
        Err(error) => report(messages, error),
    }
}

fn report(messages: &UnboundedSender<Message>, error: ApiError) {
    let _ = messages.send(Message::Failure(secrets::redact(&error.to_string())));
}

fn load_snapshot(paths: &Paths) -> monochrome_core::SyncDocument {
    std::fs::read_to_string(&paths.snapshot)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save_snapshot(app: &App, paths: &Paths) {
    if let Ok(body) = serde_json::to_vec(app.library.document()) {
        let _ = monochrome_tui::paths::write_private(&paths.snapshot, &body);
    }
}

async fn flush_on_exit(app: &mut App, services: &Arc<Services>, paths: &Paths) {
    save_snapshot(app, paths);
    let changes = app.library.take_dirty();
    if changes.is_empty() {
        return;
    }
    let Some(token) = services.secrets.get(SESSION_TOKEN) else {
        return;
    };
    let _ = tokio::time::timeout(
        Duration::from_secs(5),
        services.auth.push_sync(&token, &changes),
    )
    .await;
}

fn forward_audio_events(
    source: std::sync::mpsc::Receiver<monochrome_audio::Event>,
    messages: UnboundedSender<Message>,
) {
    std::thread::Builder::new()
        .name("monochrome-audio-bridge".into())
        .spawn(move || {
            while let Ok(event) = source.recv() {
                let message = match event {
                    monochrome_audio::Event::Loading => continue,
                    monochrome_audio::Event::Started {
                        duration, codec, ..
                    } => Message::PlaybackStarted {
                        duration,
                        format: codec,
                    },
                    monochrome_audio::Event::Position(position) => {
                        Message::PlaybackPosition(position)
                    }
                    monochrome_audio::Event::Paused(paused) => Message::PlaybackPaused(paused),
                    monochrome_audio::Event::Finished => Message::PlaybackFinished,
                    monochrome_audio::Event::Output { .. } => continue,
                    monochrome_audio::Event::Stopped => continue,
                    monochrome_audio::Event::Failed(reason) => {
                        Message::PlaybackFailed(secrets::redact(&reason))
                    }
                };
                if messages.send(message).is_err() {
                    break;
                }
            }
        })
        .expect("audio bridge starts");
}

type Backend = CrosstermBackend<std::io::Stdout>;

fn enter_terminal() -> Result<Terminal<Backend>> {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), LeaveAlternateScreen);
        previous(info);
    }));

    enable_raw_mode().context("this terminal does not support raw mode")?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen)?;
    let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    Ok(terminal)
}

fn leave_terminal(mut terminal: Terminal<Backend>) -> Result<()> {
    disable_raw_mode()?;
    crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}
