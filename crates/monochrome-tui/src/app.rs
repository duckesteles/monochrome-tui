use monochrome_api::SearchResults;
use monochrome_api::auth::User;
use monochrome_core::library::{HISTORY_THRESHOLD_SECS, SyncDocument};
use monochrome_core::model::{Album, Artist, FavoriteKind, Playlist, Quality, Track};
use monochrome_core::{Library, Queue, Repeat};
use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Library,
    Search,
    Playlists,
    Recent,
    Queue,
}

impl Tab {
    pub const ALL: [Tab; 5] = [
        Tab::Library,
        Tab::Search,
        Tab::Playlists,
        Tab::Recent,
        Tab::Queue,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Library => "library",
            Tab::Search => "search",
            Tab::Playlists => "playlists",
            Tab::Recent => "recent",
            Tab::Queue => "queue",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibrarySection {
    Tracks,
    Albums,
    Artists,
}

impl LibrarySection {
    pub const ALL: [LibrarySection; 3] = [
        LibrarySection::Tracks,
        LibrarySection::Albums,
        LibrarySection::Artists,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LibrarySection::Tracks => "tracks",
            LibrarySection::Albums => "albums",
            LibrarySection::Artists => "artists",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ArtistPage {
    pub artist: Artist,
    pub albums: Vec<Album>,
    pub top_tracks: Vec<Track>,
}

#[derive(Debug, Clone)]
pub enum Screen {
    Album(Album),
    Artist(Box<ArtistPage>),
    Playlist(Playlist, Vec<Track>),
    Loading(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    Section(String),
    Track(Track),
    Album(Album),
    Artist(Artist),
    Playlist(Playlist),
    Empty(String),
}

impl Row {
    pub fn selectable(&self) -> bool {
        !matches!(self, Row::Section(_) | Row::Empty(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Browsing,
    SearchInput,
    Login,
    Verification,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoginField {
    #[default]
    Email,
    Password,
}

#[derive(Debug, Default, Clone)]
pub struct LoginForm {
    pub email: String,
    pub password: String,
    pub field: LoginField,
    pub submitting: bool,
}

#[derive(Debug, Default, Clone)]
pub struct NowPlaying {
    pub track: Option<Track>,
    pub position: f64,
    pub duration: Option<f64>,
    pub paused: bool,
    pub loading: bool,
    pub source: Option<String>,
    pub format: Option<String>,
    pub recorded: bool,
    pub chosen_by_hand: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Search(String),
    LoadAlbum(u64),
    LoadArtist(u64),
    LoadPlaylist(String),
    LoadTrackDetails(Vec<u64>),
    Play(Box<Track>),
    Pause,
    Resume,
    Stop,
    Seek(f64),
    Volume(f32),
    PushSync,
    SignIn { email: String, password: String },
    SignOut,
    UseToken(String),
    OpenBrowser,
    Quit,
}

#[derive(Debug, Clone)]
pub enum Message {
    SearchResults(Box<SearchResults>),
    Album(Box<Album>),
    Artist(Box<ArtistPage>),
    Playlist(Box<Playlist>, Vec<Track>),
    TrackDetails(Vec<Track>),
    Sync(Box<SyncDocument>),
    SignedIn(Box<User>),
    SignInFailed(String),
    SignedOut,
    Notice(String),
    Failure(String),
    StreamReady {
        source: String,
    },
    NeedsVerification(String),
    Verified,
    VerificationFailed(String),
    PlaybackStarted {
        duration: Option<f64>,
        format: String,
    },
    PlaybackPosition(f64),
    PlaybackPaused(bool),
    PlaybackFinished,
    PlaybackFailed(String),
}

pub struct App {
    pub tab: Tab,
    pub section: LibrarySection,
    pub stack: Vec<Screen>,
    pub cursors: Vec<usize>,
    pub library: Library,
    pub queue: Queue,
    pub search_input: String,
    pub search_results: SearchResults,
    pub search_pending: bool,
    pub now: NowPlaying,
    pub status: Option<String>,
    pub focus: Focus,
    pub login: LoginForm,
    pub user: Option<User>,
    pub quality: Quality,
    pub volume: f32,
    pub muted_from: Option<f32>,
    pub verification_url: Option<String>,
    pub show_help: bool,
    pub help_scroll: u16,
    pub roomy_rows: bool,
    lengths: HashMap<u64, u32>,
    asked_about: HashSet<u64>,
    pub verification_input: String,
    pub verification_error: Option<String>,
    pub syncing: bool,
    pub quit: bool,
    clock: fn() -> u64,
}

impl App {
    pub fn new(quality: Quality, volume: f32) -> Self {
        Self {
            tab: Tab::Library,
            section: LibrarySection::Tracks,
            stack: Vec::new(),
            cursors: vec![0],
            library: Library::default(),
            queue: Queue::new(),
            search_input: String::new(),
            search_results: SearchResults::default(),
            search_pending: false,
            now: NowPlaying::default(),
            status: None,
            focus: Focus::Browsing,
            login: LoginForm::default(),
            user: None,
            quality,
            volume: volume.clamp(0.0, 1.0),
            muted_from: None,
            show_help: false,
            help_scroll: 0,
            roomy_rows: false,
            lengths: HashMap::new(),
            asked_about: HashSet::new(),
            verification_url: None,
            verification_input: String::new(),
            verification_error: None,
            syncing: false,
            quit: false,
            clock: now_ms,
        }
    }

    #[cfg(test)]
    pub fn with_clock(quality: Quality, volume: f32, clock: fn() -> u64) -> Self {
        let mut app = Self::new(quality, volume);
        app.clock = clock;
        app
    }

    pub fn signed_in(&self) -> bool {
        self.user.is_some()
    }

    pub fn cursor(&self) -> usize {
        *self.cursors.last().unwrap_or(&0)
    }

    fn set_cursor(&mut self, value: usize) {
        if let Some(cursor) = self.cursors.last_mut() {
            *cursor = value;
        }
    }

    pub fn breadcrumb(&self) -> String {
        let mut parts = vec![self.tab.label().to_string()];
        for screen in &self.stack {
            parts.push(match screen {
                Screen::Album(album) => album.title.clone(),
                Screen::Artist(page) => page.artist.name.clone(),
                Screen::Playlist(playlist, _) => playlist.title.clone(),
                Screen::Loading(label) => label.clone(),
            });
        }
        parts.join(" > ")
    }

    pub fn rows(&self) -> Vec<Row> {
        match self.stack.last() {
            Some(Screen::Loading(_)) => vec![Row::Empty("loading".into())],
            Some(Screen::Album(album)) => {
                if album.tracks.is_empty() {
                    vec![Row::Empty("this album has no playable tracks".into())]
                } else {
                    album.tracks.iter().cloned().map(Row::Track).collect()
                }
            }
            Some(Screen::Playlist(_, tracks)) => {
                if tracks.is_empty() {
                    vec![Row::Empty("this playlist is empty".into())]
                } else {
                    tracks.iter().cloned().map(Row::Track).collect()
                }
            }
            Some(Screen::Artist(page)) => {
                let mut rows = Vec::new();
                if !page.top_tracks.is_empty() {
                    rows.push(Row::Section("top tracks".into()));
                    rows.extend(page.top_tracks.iter().cloned().map(Row::Track));
                }
                if !page.albums.is_empty() {
                    rows.push(Row::Section("albums".into()));
                    rows.extend(page.albums.iter().cloned().map(Row::Album));
                }
                if rows.is_empty() {
                    rows.push(Row::Empty("nothing to show for this artist".into()));
                }
                rows
            }
            None => self.root_rows(),
        }
    }

    fn root_rows(&self) -> Vec<Row> {
        match self.tab {
            Tab::Library => match self.section {
                LibrarySection::Tracks => rows_or_empty(
                    self.library
                        .favorite_tracks()
                        .into_iter()
                        .map(|track| Row::Track(self.with_known_length(track))),
                    "no saved tracks yet",
                ),
                LibrarySection::Albums => rows_or_empty(
                    self.library.favorite_albums().into_iter().map(Row::Album),
                    "no saved albums yet",
                ),
                LibrarySection::Artists => rows_or_empty(
                    self.library.favorite_artists().into_iter().map(Row::Artist),
                    "no saved artists yet",
                ),
            },
            Tab::Playlists => rows_or_empty(
                self.library
                    .favorite_playlists()
                    .into_iter()
                    .map(Row::Playlist),
                "no saved playlists yet",
            ),
            Tab::Recent => rows_or_empty(
                self.library
                    .history()
                    .into_iter()
                    .map(|track| Row::Track(self.with_known_length(track))),
                "nothing played yet",
            ),
            Tab::Queue => rows_or_empty(
                self.queue
                    .items()
                    .iter()
                    .cloned()
                    .map(|track| Row::Track(self.with_known_length(track))),
                "the queue is empty",
            ),
            Tab::Search => {
                if self.search_pending {
                    return vec![Row::Empty("searching".into())];
                }
                if self.search_results.is_empty() {
                    let hint = if self.search_input.is_empty() {
                        "press / to search"
                    } else {
                        "nothing found"
                    };
                    return vec![Row::Empty(hint.into())];
                }
                let mut rows = Vec::new();
                if !self.search_results.tracks.is_empty() {
                    rows.push(Row::Section("tracks".into()));
                    rows.extend(self.search_results.tracks.iter().cloned().map(Row::Track));
                }
                if !self.search_results.albums.is_empty() {
                    rows.push(Row::Section("albums".into()));
                    rows.extend(self.search_results.albums.iter().cloned().map(Row::Album));
                }
                if !self.search_results.artists.is_empty() {
                    rows.push(Row::Section("artists".into()));
                    rows.extend(self.search_results.artists.iter().cloned().map(Row::Artist));
                }
                if !self.search_results.playlists.is_empty() {
                    rows.push(Row::Section("playlists".into()));
                    rows.extend(
                        self.search_results
                            .playlists
                            .iter()
                            .cloned()
                            .map(Row::Playlist),
                    );
                }
                rows
            }
        }
    }

    fn with_known_length(&self, mut track: Track) -> Track {
        if track.duration == 0
            && let Some(length) = self.lengths.get(&track.id)
        {
            track.duration = *length;
        }
        track
    }

    pub fn tracks_missing_a_length(&mut self, limit: usize) -> Vec<u64> {
        let wanted: Vec<u64> = self
            .rows()
            .into_iter()
            .filter_map(|row| match row {
                Row::Track(track) if track.duration == 0 => Some(track.id),
                _ => None,
            })
            .filter(|id| !self.asked_about.contains(id))
            .take(limit)
            .collect();
        for id in &wanted {
            self.asked_about.insert(*id);
        }
        wanted
    }

    pub fn selected_row(&self) -> Option<Row> {
        self.rows().into_iter().nth(self.cursor())
    }

    pub fn move_cursor(&mut self, delta: isize) {
        let rows = self.rows();
        if rows.is_empty() {
            return;
        }
        let mut index = self.cursor() as isize;
        let step = if delta >= 0 { 1 } else { -1 };
        let mut remaining = delta.abs();
        while remaining > 0 {
            let next = index + step;
            if next < 0 || next as usize >= rows.len() {
                break;
            }
            index = next;
            if rows[index as usize].selectable() {
                remaining -= 1;
            }
        }
        while index >= 0 && (index as usize) < rows.len() && !rows[index as usize].selectable() {
            index += step;
        }
        if index < 0 || index as usize >= rows.len() {
            index = rows
                .iter()
                .position(Row::selectable)
                .map(|position| position as isize)
                .unwrap_or(0);
        }
        self.set_cursor(index.max(0) as usize);
    }

    pub fn cursor_to_start(&mut self) {
        self.set_cursor(0);
        self.move_cursor(0);
    }

    pub fn cursor_to_end(&mut self) {
        let rows = self.rows();
        self.set_cursor(rows.len().saturating_sub(1));
        self.move_cursor(0);
    }

    pub fn switch_tab(&mut self, tab: Tab) {
        if self.tab == tab && self.stack.is_empty() {
            return;
        }
        self.tab = tab;
        self.stack.clear();
        self.cursors = vec![0];
        self.move_cursor(0);
    }

    pub fn next_tab(&mut self, forward: bool) {
        let index = Tab::ALL
            .iter()
            .position(|tab| *tab == self.tab)
            .unwrap_or(0);
        let count = Tab::ALL.len();
        let next = if forward {
            (index + 1) % count
        } else {
            (index + count - 1) % count
        };
        self.switch_tab(Tab::ALL[next]);
    }

    pub fn cycle_section(&mut self, forward: bool) {
        if self.tab != Tab::Library || !self.stack.is_empty() {
            return;
        }
        let index = LibrarySection::ALL
            .iter()
            .position(|section| *section == self.section)
            .unwrap_or(0);
        let count = LibrarySection::ALL.len();
        self.section = if forward {
            LibrarySection::ALL[(index + 1) % count]
        } else {
            LibrarySection::ALL[(index + count - 1) % count]
        };
        self.set_cursor(0);
        self.move_cursor(0);
    }

    pub fn push(&mut self, screen: Screen) {
        self.stack.push(screen);
        self.cursors.push(0);
        self.move_cursor(0);
    }

    pub fn pop(&mut self) -> bool {
        if self.stack.pop().is_some() {
            self.cursors.pop();
            if self.cursors.is_empty() {
                self.cursors.push(0);
            }
            return true;
        }
        false
    }

    pub fn open_selected(&mut self) -> Vec<Effect> {
        let Some(row) = self.selected_row() else {
            return Vec::new();
        };
        match row {
            Row::Track(track) => self.play_from_current_screen(&track),
            Row::Album(album) => {
                let id = album.id;
                self.push(Screen::Loading(album.title.clone()));
                vec![Effect::LoadAlbum(id)]
            }
            Row::Artist(artist) => {
                let id = artist.id;
                self.push(Screen::Loading(artist.name.clone()));
                vec![Effect::LoadArtist(id)]
            }
            Row::Playlist(playlist) => {
                let uuid = playlist.uuid.clone();
                self.push(Screen::Loading(playlist.title.clone()));
                vec![Effect::LoadPlaylist(uuid)]
            }
            Row::Section(_) | Row::Empty(_) => Vec::new(),
        }
    }

    fn play_from_current_screen(&mut self, track: &Track) -> Vec<Effect> {
        let tracks: Vec<Track> = self
            .rows()
            .into_iter()
            .filter_map(|row| match row {
                Row::Track(track) => Some(track),
                _ => None,
            })
            .collect();
        let start = tracks
            .iter()
            .position(|candidate| candidate.id == track.id)
            .unwrap_or(0);
        let seed = (self.clock)();
        self.queue.replace(tracks, start, seed);
        self.start_current(true)
    }

    fn start_current(&mut self, chosen_by_hand: bool) -> Vec<Effect> {
        match self.queue.current().cloned() {
            Some(track) => {
                let length = (track.duration > 0).then_some(track.duration as f64);
                self.now = NowPlaying {
                    track: Some(track.clone()),
                    duration: length,
                    loading: true,
                    chosen_by_hand,
                    ..Default::default()
                };
                vec![Effect::Play(Box::new(track))]
            }
            None => vec![Effect::Stop],
        }
    }

    pub fn play_next(&mut self, manual: bool) -> Vec<Effect> {
        if self.queue.next(manual).is_some() {
            self.start_current(manual)
        } else {
            self.now = NowPlaying::default();
            vec![Effect::Stop]
        }
    }

    pub fn play_previous(&mut self) -> Vec<Effect> {
        if self.now.position > 3.0 {
            return vec![Effect::Seek(0.0)];
        }
        if self.queue.previous().is_some() {
            self.start_current(true)
        } else {
            Vec::new()
        }
    }

    pub fn toggle_playback(&mut self) -> Vec<Effect> {
        if self.now.track.is_none() {
            return self.open_selected();
        }
        self.now.paused = !self.now.paused;
        if self.now.paused {
            vec![Effect::Pause]
        } else {
            vec![Effect::Resume]
        }
    }

    pub fn seek_by(&mut self, delta: f64) -> Vec<Effect> {
        if self.now.track.is_none() {
            return Vec::new();
        }
        let duration = self.now.duration.unwrap_or(f64::MAX);
        let target = (self.now.position + delta).clamp(0.0, duration);
        self.now.position = target;
        vec![Effect::Seek(target)]
    }

    pub fn change_volume(&mut self, delta: f32) -> Vec<Effect> {
        self.volume = (self.volume + delta).clamp(0.0, 1.0);
        self.muted_from = None;
        vec![Effect::Volume(self.volume)]
    }

    pub fn toggle_mute(&mut self) -> Vec<Effect> {
        match self.muted_from.take() {
            Some(previous) => {
                self.volume = previous;
                self.status = Some("unmuted".into());
            }
            None => {
                if self.volume > 0.0 {
                    self.muted_from = Some(self.volume);
                }
                self.volume = 0.0;
                self.status = Some("muted".into());
            }
        }
        vec![Effect::Volume(self.volume)]
    }

    pub fn queue_selected(&mut self) {
        if let Some(Row::Track(track)) = self.selected_row() {
            let title = track.display_title();
            self.queue.append(track);
            self.status = Some(format!("queued {title}"));
        }
    }

    pub fn toggle_favorite(&mut self) -> Vec<Effect> {
        let Some(row) = self.selected_row() else {
            return Vec::new();
        };
        let stamp = (self.clock)();
        let message = match row {
            Row::Track(track) => {
                let key = track.id.to_string();
                let added = !self.library.is_favorite(FavoriteKind::Track, &key);
                self.library.set_favorite_track(&track, added, stamp);
                describe(added, &track.display_title())
            }
            Row::Album(album) => {
                let key = album.id.to_string();
                let added = !self.library.is_favorite(FavoriteKind::Album, &key);
                self.library.set_favorite_album(&album, added, stamp);
                describe(added, &album.title)
            }
            Row::Artist(artist) => {
                let key = artist.id.to_string();
                let added = !self.library.is_favorite(FavoriteKind::Artist, &key);
                self.library.set_favorite_artist(&artist, added, stamp);
                describe(added, &artist.name)
            }
            Row::Playlist(playlist) => {
                let added = !self
                    .library
                    .is_favorite(FavoriteKind::Playlist, &playlist.uuid);
                self.library.set_favorite_playlist(&playlist, added, stamp);
                describe(added, &playlist.title)
            }
            Row::Section(_) | Row::Empty(_) => return Vec::new(),
        };
        self.status = Some(message);
        vec![Effect::PushSync]
    }

    pub fn toggle_shuffle(&mut self) {
        let enabled = self.queue.toggle_shuffle();
        self.status = Some(if enabled {
            "shuffle on".into()
        } else {
            "shuffle off".into()
        });
    }

    pub fn cycle_repeat(&mut self) {
        let repeat = self.queue.cycle_repeat();
        self.status = Some(repeat.label().to_string());
    }

    pub fn repeat(&self) -> Repeat {
        self.queue.repeat()
    }

    pub fn toggle_help(&mut self) {
        self.show_help = !self.show_help;
        self.help_scroll = 0;
        self.status = None;
    }

    pub fn scroll_help(&mut self, delta: i16) {
        let lines = crate::help::line_count() as u16;
        let furthest = lines.saturating_sub(1);
        self.help_scroll =
            (self.help_scroll as i32 + delta as i32).clamp(0, furthest as i32) as u16;
    }

    pub fn submit_verification(&mut self) -> Vec<Effect> {
        let pasted = clean_pasted_token(&self.verification_input);
        if pasted.is_empty() {
            return Vec::new();
        }
        if matches!(pasted.as_str(), "null" | "undefined") {
            self.verification_input.clear();
            self.verification_error = Some(
                "the web app has no token yet, so the console printed nothing. play something on \
                 monochrome.tf first, then copy it again."
                    .into(),
            );
            return Vec::new();
        }
        self.verification_input.clear();
        vec![Effect::UseToken(pasted)]
    }

    pub fn submit_search(&mut self) -> Vec<Effect> {
        let query = self.search_input.trim().to_string();
        self.focus = Focus::Browsing;
        if query.is_empty() {
            return Vec::new();
        }
        self.tab = Tab::Search;
        self.stack.clear();
        self.cursors = vec![0];
        self.search_pending = true;
        self.search_results = SearchResults::default();
        vec![Effect::Search(query)]
    }

    pub fn apply(&mut self, message: Message) -> Vec<Effect> {
        match message {
            Message::SearchResults(results) => {
                self.search_pending = false;
                self.search_results = *results;
                self.cursor_to_start();
                Vec::new()
            }
            Message::Album(album) => {
                self.replace_top(Screen::Album(*album));
                Vec::new()
            }
            Message::Artist(page) => {
                self.replace_top(Screen::Artist(page));
                Vec::new()
            }
            Message::TrackDetails(details) => {
                for track in details {
                    if track.duration > 0 {
                        self.lengths.insert(track.id, track.duration);
                    }
                }
                Vec::new()
            }
            Message::Playlist(playlist, tracks) => {
                self.replace_top(Screen::Playlist(*playlist, tracks));
                Vec::new()
            }
            Message::Sync(document) => {
                self.syncing = false;
                self.library.merge_remote(*document);
                self.move_cursor(0);
                Vec::new()
            }
            Message::SignedIn(user) => {
                self.user = Some(*user);
                self.focus = Focus::Browsing;
                self.login = LoginForm::default();
                self.status = Some("signed in".into());
                Vec::new()
            }
            Message::SignInFailed(reason) => {
                self.login.submitting = false;
                self.login.password.clear();
                self.focus = Focus::Login;
                self.status = Some(reason);
                Vec::new()
            }
            Message::SignedOut => {
                self.user = None;
                self.library = Library::default();
                self.queue.clear();
                self.now = NowPlaying::default();
                self.focus = Focus::Login;
                Vec::new()
            }
            Message::Notice(text) | Message::Failure(text) => {
                self.status = Some(text);
                Vec::new()
            }
            Message::StreamReady { source } => {
                self.now.source = Some(source);
                Vec::new()
            }
            Message::NeedsVerification(url) => {
                self.verification_url = Some(url);
                self.verification_error = None;
                self.focus = Focus::Verification;
                Vec::new()
            }
            Message::VerificationFailed(reason) => {
                self.verification_error = Some(reason);
                self.focus = Focus::Verification;
                Vec::new()
            }
            Message::Verified => {
                self.verification_url = None;
                self.verification_input.clear();
                self.verification_error = None;
                self.focus = Focus::Browsing;
                self.status = Some("verified".into());
                match self.queue.current().cloned() {
                    Some(track) => vec![Effect::Play(Box::new(track))],
                    None => Vec::new(),
                }
            }
            Message::PlaybackStarted { duration, format } => {
                self.now.loading = false;
                self.now.paused = false;
                self.now.recorded = false;
                self.now.format = Some(format);
                if let Some(duration) = duration {
                    self.now.duration = Some(duration);
                }
                Vec::new()
            }
            Message::PlaybackPosition(position) => {
                self.now.position = position;
                self.record_history_if_due()
            }
            Message::PlaybackPaused(paused) => {
                self.now.paused = paused;
                Vec::new()
            }
            Message::PlaybackFinished => self.play_next(false),
            Message::PlaybackFailed(reason) => {
                self.status = Some(reason);
                self.now.loading = false;
                if self.now.chosen_by_hand || !self.queue.has_next() {
                    self.now = NowPlaying::default();
                    Vec::new()
                } else {
                    self.play_next(false)
                }
            }
        }
    }

    fn record_history_if_due(&mut self) -> Vec<Effect> {
        if self.now.recorded || self.now.position < HISTORY_THRESHOLD_SECS as f64 {
            return Vec::new();
        }
        let Some(track) = self.now.track.clone() else {
            return Vec::new();
        };
        self.now.recorded = true;
        self.library.record_play(&track, (self.clock)());
        vec![Effect::PushSync]
    }

    fn replace_top(&mut self, screen: Screen) {
        if matches!(self.stack.last(), Some(Screen::Loading(_))) {
            self.stack.pop();
            self.cursors.pop();
        }
        self.push(screen);
    }
}

pub fn clean_pasted_token(raw: &str) -> String {
    raw.trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == '`')
        .trim()
        .to_string()
}

fn describe(added: bool, title: &str) -> String {
    if added {
        format!("saved {title}")
    } else {
        format!("removed {title}")
    }
}

fn rows_or_empty(rows: impl Iterator<Item = Row>, empty: &str) -> Vec<Row> {
    let collected: Vec<Row> = rows.collect();
    if collected.is_empty() {
        vec![Row::Empty(empty.into())]
    } else {
        collected
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or_default()
}
