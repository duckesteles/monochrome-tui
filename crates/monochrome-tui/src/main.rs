use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{Event, EventStream, KeyEventKind};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures::StreamExt;
use monochrome_api::error::ApiError;
use monochrome_api::{AuthClient, Catalog, StreamResolver};
use monochrome_audio::{PlayRequest, Player};
use monochrome_core::model::Track;
use monochrome_tui::app::{App, ArtistPage, Effect, Focus, Message};
use monochrome_tui::banner::Banner;
use monochrome_tui::config::{Config, Paths};
use monochrome_tui::dispatch;
use monochrome_tui::secrets::{self, PLAYBACK_SESSION, SESSION_TOKEN, Secrets};
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
const RESTORE_ATTEMPTS: u32 = 3;
const RESTORE_PAUSE: Duration = Duration::from_secs(2);

#[derive(Parser, Debug)]
#[command(
    name = "monochrome",
    about = "A terminal client for monochrome.tf",
    version
)]
struct Args {
    #[arg(
        long,
        help = "Log everything, not only what went wrong, to the state directory"
    )]
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
    #[arg(long, help = "Remove the program and everything it stored, then exit")]
    uninstall: bool,
    #[arg(long, help = "Answer yes to the uninstall prompt")]
    yes: bool,
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

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;

    if args.uninstall {
        return uninstall(paths, args.yes);
    }

    if args.doctor {
        return runtime.block_on(monochrome_tui::diagnostics::doctor(paths));
    }
    if let Some(query) = args.probe {
        return runtime.block_on(monochrome_tui::diagnostics::probe(paths, query));
    }
    if let Some(query) = args.play {
        return runtime.block_on(monochrome_tui::diagnostics::play_once(paths, query));
    }

    let _guard = setup_logging(&paths.log_dir, args.verbose).ok();
    runtime.block_on(run(paths))
}

fn uninstall(paths: Paths, assume_yes: bool) -> Result<()> {
    use monochrome_tui::uninstall::{execute, plan};

    let binary = std::env::current_exe().ok();
    let targets = plan(&paths, binary);

    println!("This removes monochrome and everything it stored:");
    println!();
    for target in &targets {
        let mark = if target.exists() { " " } else { "-" };
        println!("  {mark} {}", target.describe());
    }
    println!();

    if !assume_yes {
        print!("Type yes to go ahead: ");
        use std::io::Write;
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if answer.trim() != "yes" {
            println!("Nothing was removed.");
            return Ok(());
        }
        println!();
    }

    let secrets = Secrets::new(paths.log_dir.join("credentials"));
    for (what, done) in execute(&targets, &secrets) {
        println!("  {} {what}", if done { "removed" } else { "FAILED " });
    }

    println!();
    println!("Done. Nothing of monochrome is left on this machine.");
    Ok(())
}

fn setup_logging(
    dir: &std::path::Path,
    verbose: bool,
) -> Result<tracing_appender::non_blocking::WorkerGuard> {
    monochrome_tui::paths::create_private_dir(dir)?;
    monochrome_tui::paths::create_private_file(&dir.join("log"))?;
    let appender = tracing_appender::rolling::never(dir, "log");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(
                |_| match verbose {
                    true => "monochrome=debug,info".into(),
                    false => "warn".into(),
                },
            ),
        )
        .init();
    Ok(guard)
}

async fn run(paths: Paths) -> Result<()> {
    let mut config = Config::load(&paths.config)?;
    if !paths.config.exists() {
        config.save(&paths.config).ok();
    }

    let secrets = Secrets::new(paths.log_dir.join("credentials"));
    let resolver = StreamResolver::new(config.stream_config())?;
    if let Some(stored) = secrets.get(PLAYBACK_SESSION) {
        resolver.restore_session(&stored);
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
    app.roomy_rows = config.roomy_rows();
    app.library = load_snapshot(&paths);

    match services.secrets.get(SESSION_TOKEN) {
        Some(token) => restore_account(services.clone(), token, messages.clone()),
        None => app.focus = Focus::Login,
    }

    let mut terminal = enter_terminal()?;
    let theme = Theme::new(&config.ui.accent);
    let mut list = ListState::default();
    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(500));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut banner = Banner::new(STATUS_LIFETIME, Instant::now());
    let mut bridge_offered = false;
    let mut scheduler = SyncScheduler::new(SYNC_DEBOUNCE);
    let mut redraw = true;
    let mut trouble = None;

    loop {
        if redraw {
            if let Err(error) = terminal.draw(|frame| views::render(frame, &app, &theme, &mut list))
            {
                trouble = Some(error);
                break;
            }
            redraw = false;
        }

        let effects = tokio::select! {
            event = events.next() => match event {
                Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                    redraw = true;
                    dispatch::on_key(&mut app, key)
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
                    match &message {
                        Message::NeedsVerification(_) => bridge_offered = true,
                        Message::Verified | Message::VerificationFailed(_) => bridge_offered = false,
                        _ => {}
                    }
                    match &message {
                        Message::Sync(_) => scheduler.settled(),
                        Message::SyncRejected { temporary: true, .. } => {
                            scheduler.retry(Instant::now())
                        }
                        _ => {}
                    }
                    let arrived_from_server =
                        matches!(message, Message::Sync(_) | Message::SyncRejected { .. });
                    let signed_out = matches!(message, Message::SignedOut);
                    let effects = app.apply(message);
                    if arrived_from_server {
                        save_snapshot(&app, &paths);
                    }
                    if signed_out {
                        forget_everything_local(&services, &paths);
                    }
                    effects
                }
                None => break,
            },
            _ = ticker.tick() => {
                if banner.is_stale(Instant::now()) {
                    app.status = None;
                    redraw = true;
                }
                if scheduler.take_if_due(Instant::now()) {
                    push_sync(&mut app, &services, &messages, &paths);
                    redraw = true;
                }
                match app.tracks_missing_a_length(24) {
                    wanted if wanted.is_empty() => Vec::new(),
                    wanted => vec![Effect::LoadTrackDetails(wanted)],
                }
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
        banner.watch(app.status.as_deref(), Instant::now());

        if app.quit {
            break;
        }
    }

    let anything_undelivered = !app.library.dirty_fields().is_empty();
    if scheduler.take_now() || anything_undelivered {
        flush_on_exit(&mut app, &services, &paths).await;
    } else {
        save_snapshot(&app, &paths);
    }
    if (app.volume - config.volume()).abs() > f32::EPSILON {
        config.playback.volume = app.volume;
        let _ = config.save(&paths.config);
    }
    leave_terminal(terminal)?;
    match trouble {
        Some(error) => Err(error).context("the terminal stopped accepting output"),
        None => Ok(()),
    }
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
                let message = match services.catalog.search(&query).await {
                    Ok(results) => Message::SearchResults(Box::new(results)),
                    Err(error) => {
                        let reason = secrets::redact(&error.to_string());
                        tracing::warn!(reason, "the search failed");
                        Message::SearchFailed(format!("the search failed: {reason}"))
                    }
                };
                let _ = messages.send(message);
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
                    Err(error) => report(&messages, "could not open that album", error),
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
                    Err(error) => report(&messages, "could not open that artist", error),
                }
            });
        }
        Effect::LoadTrackDetails(ids) => {
            let services = services.clone();
            let messages = messages.clone();
            tokio::spawn(async move {
                let details = services.catalog.tracks(&ids).await;
                tracing::debug!(
                    asked = ids.len(),
                    learned = details.len(),
                    "filled in track lengths"
                );
                if !details.is_empty() {
                    let _ = messages.send(Message::TrackDetails(details));
                }
            });
        }
        Effect::Radio(track_id) => {
            let services = services.clone();
            let messages = messages.clone();
            tokio::spawn(async move {
                match services.catalog.recommendations(track_id).await {
                    Ok(tracks) if tracks.is_empty() => {
                        let _ = messages.send(Message::Notice(
                            "the catalogue knows nothing like that track".into(),
                        ));
                    }
                    Ok(tracks) => {
                        let _ = messages.send(Message::Radio(tracks));
                    }
                    Err(error) => report(&messages, "could not build a radio", error),
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
                    Err(error) => report(&messages, "could not open that playlist", error),
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
                let (replay_gain, peak) = handle.levelling(&track);
                player.play(PlayRequest {
                    url: handle.url,
                    headers: handle.headers,
                    replay_gain,
                    peak,
                    decryption_key: handle.decryption_key,
                    expected_duration: (track.duration > 0).then_some(track.duration as f64),
                });
            }
            Err(ApiError::CredentialRejected) => {
                let _ = messages.send(Message::VerificationFailed(
                    "the playback session expired. the browser check will get a new one.".into(),
                ));
            }
            Err(ApiError::TurnstileRequired) if !allow_bridge => {
                let _ = messages.send(Message::VerificationFailed(
                    "the playback service still refuses this session. try the browser check \
                     again."
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
                                        persist_playback_session(&services);
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
                    Err(error) => report(&messages, "could not start the browser check", error),
                }
            }
            Err(error) => {
                let _ = messages.send(Message::PlaybackFailed {
                    worth_moving_on: monochrome_api::stream::worth_moving_on(&error),
                    reason: secrets::redact(&error.to_string()),
                });
            }
        }
    });
}

fn persist_playback_session(services: &Arc<Services>) {
    if let Some(record) = services.resolver.session_for_storage() {
        let _ = services.secrets.set(PLAYBACK_SESSION, &record);
    }
}

fn push_sync(
    app: &mut App,
    services: &Arc<Services>,
    messages: &UnboundedSender<Message>,
    paths: &Paths,
) {
    save_snapshot(app, paths);
    if app.library.dirty_fields().is_empty() {
        return;
    }
    let Some(token) = services.secrets.get(SESSION_TOKEN) else {
        return;
    };
    let changes = app.library.take_dirty();
    app.syncing = true;
    let services = services.clone();
    let messages = messages.clone();
    let fields: Vec<_> = changes.iter().map(|(field, _)| *field).collect();
    tokio::spawn(async move {
        match services.auth.push_sync(&token, &changes).await {
            Ok(document) => {
                let _ = messages.send(Message::Sync(Box::new(document)));
            }
            Err(ApiError::Unauthorized) => {
                services.secrets.clear(SESSION_TOKEN);
                let _ = messages.send(Message::SignedOut);
            }
            Err(error) => {
                let reason = secrets::redact(&error.to_string());
                tracing::warn!(
                    reason,
                    "the account service would not take the library changes"
                );
                let _ = messages.send(Message::SyncRejected {
                    fields,
                    reason,
                    temporary: error.is_temporary(),
                });
            }
        }
    });
}

fn restore_account(services: Arc<Services>, token: String, messages: UnboundedSender<Message>) {
    tokio::spawn(async move {
        let mut wait = RESTORE_PAUSE;
        for attempt in 1..=RESTORE_ATTEMPTS {
            match services.auth.me(&token).await {
                Ok(user) => {
                    let _ = messages.send(Message::SignedIn(Box::new(user)));
                    load_sync(&services, &token, &messages).await;
                    return;
                }
                Err(ApiError::Unauthorized) => {
                    services.secrets.clear(SESSION_TOKEN);
                    let _ = messages.send(Message::SignedOut);
                    return;
                }
                Err(error) if error.is_temporary() && attempt < RESTORE_ATTEMPTS => {
                    tracing::warn!(
                        attempt,
                        reason = secrets::redact(&error.to_string()),
                        "the account service did not answer, asking again"
                    );
                    tokio::time::sleep(wait).await;
                    wait *= 2;
                }
                Err(error) => {
                    report(&messages, "could not reach the account service", error);
                    return;
                }
            }
        }
    });
}

async fn load_sync(services: &Arc<Services>, token: &str, messages: &UnboundedSender<Message>) {
    match services.auth.load_sync(token).await {
        Ok(document) => {
            let _ = messages.send(Message::Sync(Box::new(document)));
        }
        Err(error) => report(messages, "could not load your saved library", error),
    }
}

fn report(messages: &UnboundedSender<Message>, doing: &str, error: ApiError) {
    let reason = secrets::redact(&error.to_string());
    tracing::warn!(doing, reason, "a background request failed");
    let _ = messages.send(Message::Failure(format!("{doing}: {reason}")));
}

#[derive(serde::Deserialize)]
struct Snapshot {
    document: monochrome_core::SyncDocument,
    #[serde(default)]
    pending: Vec<monochrome_core::library::SyncField>,
}

#[derive(serde::Serialize)]
struct Saving<'a> {
    document: &'a monochrome_core::SyncDocument,
    pending: &'a [monochrome_core::library::SyncField],
}

fn load_snapshot(paths: &Paths) -> monochrome_core::Library {
    let Ok(raw) = std::fs::read_to_string(&paths.snapshot) else {
        return monochrome_core::Library::default();
    };
    if let Ok(saved) = serde_json::from_str::<Snapshot>(&raw) {
        let mut library = monochrome_core::Library::new(saved.document);
        library.mark_dirty(&saved.pending);
        return library;
    }
    serde_json::from_str(&raw)
        .map(monochrome_core::Library::new)
        .unwrap_or_default()
}

fn forget_everything_local(services: &Arc<Services>, paths: &Paths) {
    let _ = monochrome_tui::paths::discard(&paths.snapshot);
    services.secrets.clear(SESSION_TOKEN);
    services.secrets.clear(PLAYBACK_SESSION);
}

fn save_snapshot(app: &App, paths: &Paths) {
    let saving = Saving {
        document: app.library.document(),
        pending: app.library.dirty_fields(),
    };
    if let Ok(body) = serde_json::to_vec(&saving) {
        let _ = monochrome_tui::paths::write_private(&paths.snapshot, &body);
    }
}

async fn flush_on_exit(app: &mut App, services: &Arc<Services>, paths: &Paths) {
    let changes = app.library.take_dirty();
    let fields: Vec<_> = changes.iter().map(|(field, _)| *field).collect();
    let delivered = match services.secrets.get(SESSION_TOKEN) {
        Some(token) if !changes.is_empty() => matches!(
            tokio::time::timeout(
                Duration::from_secs(5),
                services.auth.push_sync(&token, &changes),
            )
            .await,
            Ok(Ok(_))
        ),
        _ => changes.is_empty(),
    };
    if !delivered {
        app.library.mark_dirty(&fields);
    }
    save_snapshot(app, paths);
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
                    monochrome_audio::Event::Failed(reason) => Message::PlaybackFailed {
                        reason: secrets::redact(&reason),
                        worth_moving_on: false,
                    },
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

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use monochrome_core::library::SyncField;

    struct Scratch {
        directory: std::path::PathBuf,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let directory = std::env::temp_dir().join(format!(
                "monochrome-snapshot-{name}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&directory);
            Self { directory }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    fn paths_in(scratch: &Scratch) -> Paths {
        Paths {
            config: scratch.directory.join("config.toml"),
            snapshot: scratch.directory.join("snapshot.json"),
            log_dir: scratch.directory.clone(),
        }
    }

    fn favouriting(library: &mut monochrome_core::Library) {
        library.set_favorite_artist(
            &monochrome_core::model::Artist {
                id: 7,
                name: "Artist".into(),
                picture: None,
                popularity: None,
            },
            true,
            1_700_000_000_000,
        );
    }

    #[test]
    fn a_change_that_never_reached_the_server_is_still_pending_after_a_restart() {
        let scratch = Scratch::new("pending");
        let paths = paths_in(&scratch);
        let mut app = App::new(monochrome_core::model::Quality::Lossless, 0.7);
        favouriting(&mut app.library);
        assert_eq!(app.library.dirty_fields(), &[SyncField::Library]);

        save_snapshot(&app, &paths);
        let reopened = load_snapshot(&paths);
        assert_eq!(reopened.dirty_fields(), &[SyncField::Library]);
        assert!(reopened.is_favorite(monochrome_core::FavoriteKind::Artist, "7"));
    }

    #[test]
    fn a_snapshot_written_by_an_older_build_still_opens() {
        let scratch = Scratch::new("older-snapshot");
        let paths = paths_in(&scratch);
        let mut library = monochrome_core::Library::default();
        favouriting(&mut library);
        let body = serde_json::to_vec(library.document()).expect("serialise");
        monochrome_tui::paths::write_private(&paths.snapshot, &body).expect("write");

        let reopened = load_snapshot(&paths);
        assert!(reopened.is_favorite(monochrome_core::FavoriteKind::Artist, "7"));
        assert!(reopened.dirty_fields().is_empty());
    }

    #[test]
    fn a_missing_snapshot_opens_an_empty_library() {
        let scratch = Scratch::new("absent-snapshot");
        let library = load_snapshot(&paths_in(&scratch));
        assert!(library.dirty_fields().is_empty());
        assert_eq!(library.history().len(), 0);
    }
}
