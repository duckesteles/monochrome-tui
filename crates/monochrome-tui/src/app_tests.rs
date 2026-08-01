use crate::app::*;
use monochrome_api::SearchResults;
use monochrome_core::model::{
    Album, AlbumRef, Artist, ArtistRef, FavoriteKind, Playlist, Quality, Track,
};

fn clock() -> u64 {
    1_700_000_000_000
}

fn app() -> App {
    App::with_clock(Quality::Lossless, 0.7, clock)
}

fn track(id: u64) -> Track {
    Track {
        id,
        title: format!("Song {id}"),
        duration: 200,
        explicit: false,
        artist: Some(ArtistRef {
            id: 7,
            name: "Artist".into(),
            picture: None,
        }),
        artists: Vec::new(),
        album: Some(AlbumRef {
            id: 5,
            title: "Album".into(),
            cover: None,
            release_date: None,
            artist: None,
            number_of_tracks: Some(2),
        }),
        isrc: Some(format!("ISRC{id:08}")),
        track_number: Some(id as u32),
        volume_number: Some(1),
        copyright: None,
        version: None,
        quality: Quality::Lossless,
        replay_gain: None,
        peak: None,
        stream_ready: true,
    }
}

fn album(id: u64, tracks: Vec<Track>) -> Album {
    Album {
        id,
        title: format!("Album {id}"),
        cover: None,
        release_date: Some("2001-03-12".into()),
        artist: Some(ArtistRef {
            id: 7,
            name: "Artist".into(),
            picture: None,
        }),
        number_of_tracks: Some(tracks.len() as u32),
        duration: None,
        explicit: false,
        quality: Quality::Lossless,
        album_type: Some("ALBUM".into()),
        copyright: None,
        tracks,
    }
}

fn artist(id: u64) -> Artist {
    Artist {
        id,
        name: format!("Artist {id}"),
        picture: None,
        popularity: None,
    }
}

fn playlist(uuid: &str) -> Playlist {
    Playlist {
        uuid: uuid.into(),
        title: "Chill".into(),
        image: None,
        number_of_tracks: Some(3),
        duration: None,
        creator_name: Some("TIDAL".into()),
        description: None,
    }
}

fn results_with_everything() -> SearchResults {
    SearchResults {
        tracks: vec![track(1), track(2)],
        albums: vec![album(10, Vec::new())],
        artists: vec![artist(20)],
        playlists: vec![playlist("uuid-1")],
    }
}

#[test]
fn a_fresh_app_opens_on_the_library_tab() {
    let app = app();
    assert_eq!(app.tab, Tab::Library);
    assert_eq!(app.section, LibrarySection::Tracks);
    assert!(app.stack.is_empty());
    assert!(!app.signed_in());
}

#[test]
fn an_empty_library_explains_itself_instead_of_showing_nothing() {
    let app = app();
    assert_eq!(app.rows(), vec![Row::Empty("no saved tracks yet".into())]);
}

#[test]
fn tabs_cycle_in_both_directions() {
    let mut app = app();
    app.next_tab(true);
    assert_eq!(app.tab, Tab::Search);
    app.next_tab(false);
    assert_eq!(app.tab, Tab::Library);
    app.next_tab(false);
    assert_eq!(app.tab, Tab::Queue);
}

#[test]
fn library_sections_cycle_only_at_the_root() {
    let mut app = app();
    app.cycle_section(true);
    assert_eq!(app.section, LibrarySection::Albums);
    app.push(Screen::Album(album(1, Vec::new())));
    app.cycle_section(true);
    assert_eq!(app.section, LibrarySection::Albums);
}

#[test]
fn switching_tabs_drops_the_navigation_stack() {
    let mut app = app();
    app.push(Screen::Album(album(1, vec![track(1)])));
    app.switch_tab(Tab::Search);
    assert!(app.stack.is_empty());
    assert_eq!(app.breadcrumb(), "search");
}

#[test]
fn the_breadcrumb_follows_the_navigation_stack() {
    let mut app = app();
    app.push(Screen::Artist(Box::new(ArtistPage {
        artist: artist(3),
        albums: vec![album(9, Vec::new())],
        top_tracks: Vec::new(),
    })));
    app.push(Screen::Album(album(9, vec![track(1)])));
    assert_eq!(app.breadcrumb(), "library > Artist 3 > Album 9");
}

#[test]
fn escaping_walks_back_up_one_level_at_a_time() {
    let mut app = app();
    app.push(Screen::Album(album(1, vec![track(1)])));
    assert!(app.pop());
    assert!(!app.pop());
}

#[test]
fn each_screen_remembers_its_own_cursor() {
    let mut app = app();
    app.search_results = results_with_everything();
    app.tab = Tab::Search;
    app.move_cursor(2);
    let outer = app.cursor();
    app.push(Screen::Album(album(1, vec![track(1), track(2), track(3)])));
    app.move_cursor(2);
    assert_eq!(app.cursor(), 2);
    app.pop();
    assert_eq!(app.cursor(), outer);
}

#[test]
fn the_cursor_skips_over_section_headings() {
    let mut app = app();
    app.tab = Tab::Search;
    app.search_results = results_with_everything();
    let rows = app.rows();
    assert_eq!(rows[0], Row::Section("tracks".into()));
    app.cursor_to_start();
    assert!(app.selected_row().expect("row").selectable());
    for _ in 0..rows.len() + 2 {
        app.move_cursor(1);
        assert!(app.selected_row().expect("row").selectable());
    }
}

#[test]
fn the_cursor_stops_at_both_ends() {
    let mut app = app();
    app.tab = Tab::Search;
    app.search_results = results_with_everything();
    app.cursor_to_end();
    let last = app.cursor();
    app.move_cursor(5);
    assert_eq!(app.cursor(), last);
    app.cursor_to_start();
    app.move_cursor(-5);
    assert_eq!(app.cursor(), 1);
}

#[test]
fn opening_an_album_row_asks_for_that_album() {
    let mut app = app();
    app.tab = Tab::Search;
    app.search_results = SearchResults {
        albums: vec![album(42, Vec::new())],
        ..Default::default()
    };
    app.cursor_to_start();
    let effects = app.open_selected();
    assert_eq!(effects, vec![Effect::LoadAlbum(42)]);
    assert!(matches!(app.stack.last(), Some(Screen::Loading(_))));
}

#[test]
fn a_loading_screen_is_replaced_rather_than_stacked() {
    let mut app = app();
    app.tab = Tab::Search;
    app.search_results = SearchResults {
        albums: vec![album(42, Vec::new())],
        ..Default::default()
    };
    app.cursor_to_start();
    app.open_selected();
    app.apply(Message::Album(Box::new(album(42, vec![track(1)]))));
    assert_eq!(app.stack.len(), 1);
    assert_eq!(app.breadcrumb(), "search > Album 42");
}

#[test]
fn playing_a_track_queues_every_track_on_the_screen() {
    let mut app = app();
    app.push(Screen::Album(album(1, vec![track(1), track(2), track(3)])));
    app.move_cursor(1);
    let effects = app.open_selected();
    assert_eq!(app.queue.len(), 3);
    assert_eq!(app.queue.current().expect("current").id, 2);
    assert!(matches!(effects.first(), Some(Effect::Play(_))));
    assert!(app.now.loading);
}

#[test]
fn an_artist_page_lists_top_tracks_before_albums() {
    let mut app = app();
    app.push(Screen::Artist(Box::new(ArtistPage {
        artist: artist(1),
        albums: vec![album(2, Vec::new())],
        top_tracks: vec![track(1)],
    })));
    let rows = app.rows();
    assert_eq!(rows[0], Row::Section("top tracks".into()));
    assert!(matches!(rows[1], Row::Track(_)));
    assert_eq!(rows[2], Row::Section("albums".into()));
}

#[test]
fn finishing_a_track_advances_the_queue() {
    let mut app = app();
    app.push(Screen::Album(album(1, vec![track(1), track(2)])));
    app.cursor_to_start();
    app.open_selected();
    let effects = app.apply(Message::PlaybackFinished);
    assert!(matches!(effects.first(), Some(Effect::Play(_))));
    assert_eq!(app.queue.current().expect("current").id, 2);
}

#[test]
fn finishing_the_last_track_stops_playback() {
    let mut app = app();
    app.push(Screen::Album(album(1, vec![track(1)])));
    app.cursor_to_start();
    app.open_selected();
    let effects = app.apply(Message::PlaybackFinished);
    assert_eq!(effects, vec![Effect::Stop]);
    assert!(app.now.track.is_none());
}

#[test]
fn a_playback_failure_moves_on_to_the_next_track() {
    let mut app = app();
    app.push(Screen::Album(album(1, vec![track(1), track(2)])));
    app.cursor_to_start();
    app.open_selected();
    app.apply(Message::PlaybackFinished);
    let effects = app.apply(Message::PlaybackFailed("no source".into()));
    assert_eq!(app.status.as_deref(), Some("no source"));
    assert!(effects.is_empty());
}

#[test]
fn a_track_the_listener_picked_is_never_silently_replaced_by_another() {
    let mut app = app();
    app.push(Screen::Album(album(1, vec![track(1), track(2), track(3)])));
    app.cursor_to_start();
    app.open_selected();
    assert_eq!(app.queue.current().expect("current").id, 1);

    let effects = app.apply(Message::PlaybackFailed("no source".into()));
    assert!(
        effects.is_empty(),
        "picking a track and getting a different one is worse than getting nothing"
    );
    assert!(app.now.track.is_none());
    assert_eq!(app.status.as_deref(), Some("no source"));
}

#[test]
fn a_track_that_started_on_its_own_still_gives_way_to_the_next() {
    let mut app = app();
    app.push(Screen::Album(album(1, vec![track(1), track(2), track(3)])));
    app.cursor_to_start();
    app.open_selected();
    app.apply(Message::PlaybackFinished);
    assert_eq!(app.queue.current().expect("current").id, 2);

    let effects = app.apply(Message::PlaybackFailed("unavailable".into()));
    assert!(matches!(effects.first(), Some(Effect::Play(_))));
    assert_eq!(app.queue.current().expect("current").id, 3);
}

#[test]
fn a_failure_on_the_last_track_clears_the_player() {
    let mut app = app();
    app.push(Screen::Album(album(1, vec![track(1)])));
    app.cursor_to_start();
    app.open_selected();
    app.apply(Message::PlaybackFailed("no source".into()));
    assert!(app.now.track.is_none());
}

#[test]
fn history_is_written_once_the_track_passes_ten_seconds() {
    let mut app = app();
    app.push(Screen::Album(album(1, vec![track(1)])));
    app.cursor_to_start();
    app.open_selected();
    app.apply(Message::PlaybackStarted {
        duration: Some(200.0),
        format: "flac".into(),
    });

    assert!(app.apply(Message::PlaybackPosition(9.0)).is_empty());
    assert!(app.library.history().is_empty());

    let effects = app.apply(Message::PlaybackPosition(10.5));
    assert_eq!(effects, vec![Effect::PushSync]);
    assert_eq!(app.library.history().len(), 1);

    assert!(app.apply(Message::PlaybackPosition(30.0)).is_empty());
    assert_eq!(app.library.history().len(), 1);
}

#[test]
fn seeking_is_clamped_to_the_track_length() {
    let mut app = app();
    app.now.track = Some(track(1));
    app.now.duration = Some(100.0);
    app.now.position = 95.0;
    app.seek_by(30.0);
    assert_eq!(app.now.position, 100.0);
    app.seek_by(-500.0);
    assert_eq!(app.now.position, 0.0);
}

#[test]
fn seeking_does_nothing_without_a_track() {
    let mut app = app();
    assert!(app.seek_by(10.0).is_empty());
}

#[test]
fn previous_restarts_the_track_when_it_is_already_playing() {
    let mut app = app();
    app.push(Screen::Album(album(1, vec![track(1), track(2)])));
    app.cursor_to_start();
    app.open_selected();
    app.now.position = 30.0;
    assert_eq!(app.play_previous(), vec![Effect::Seek(0.0)]);
}

#[test]
fn previous_steps_back_early_in_a_track() {
    let mut app = app();
    app.push(Screen::Album(album(1, vec![track(1), track(2)])));
    app.cursor_to_start();
    app.open_selected();
    app.play_next(true);
    app.now.position = 1.0;
    app.play_previous();
    assert_eq!(app.queue.current().expect("current").id, 1);
}

#[test]
fn volume_changes_are_clamped() {
    let mut app = app();
    app.change_volume(1.0);
    assert_eq!(app.volume, 1.0);
    app.change_volume(-5.0);
    assert_eq!(app.volume, 0.0);
}

#[test]
fn muting_remembers_the_previous_volume() {
    let mut app = app();
    app.change_volume(0.1);
    let before = app.volume;
    assert_eq!(app.toggle_mute(), vec![Effect::Volume(0.0)]);
    assert_eq!(app.volume, 0.0);
    assert_eq!(app.status.as_deref(), Some("muted"));
    assert_eq!(app.toggle_mute(), vec![Effect::Volume(before)]);
    assert_eq!(app.volume, before);
    assert_eq!(app.status.as_deref(), Some("unmuted"));
}

#[test]
fn changing_the_volume_while_muted_cancels_the_mute() {
    let mut app = app();
    app.toggle_mute();
    app.change_volume(0.2);
    assert!(app.muted_from.is_none());
    app.toggle_mute();
    assert_eq!(app.volume, 0.0);
    app.toggle_mute();
    assert!((app.volume - 0.2).abs() < 0.001);
}

#[test]
fn muting_an_already_silent_player_does_not_trap_it_at_zero() {
    let mut app = app();
    app.change_volume(-1.0);
    assert_eq!(app.volume, 0.0);
    app.toggle_mute();
    app.toggle_mute();
    assert_eq!(app.volume, 0.0);
}

#[test]
fn toggling_a_favorite_marks_the_library_for_sync() {
    let mut app = app();
    app.push(Screen::Album(album(1, vec![track(1)])));
    app.cursor_to_start();
    let effects = app.toggle_favorite();
    assert_eq!(effects, vec![Effect::PushSync]);
    assert!(app.library.is_favorite(FavoriteKind::Track, "1"));
    assert_eq!(app.status.as_deref(), Some("saved Song 1"));

    app.toggle_favorite();
    assert!(!app.library.is_favorite(FavoriteKind::Track, "1"));
    assert_eq!(app.status.as_deref(), Some("removed Song 1"));
}

#[test]
fn favoriting_a_heading_does_nothing() {
    let mut app = app();
    app.tab = Tab::Search;
    app.search_results = results_with_everything();
    app.cursors = vec![0];
    assert!(app.toggle_favorite().is_empty());
}

#[test]
fn queueing_a_track_appends_without_disturbing_playback() {
    let mut app = app();
    app.push(Screen::Album(album(1, vec![track(1), track(2)])));
    app.cursor_to_start();
    app.open_selected();
    app.move_cursor(1);
    app.queue_selected();
    assert_eq!(app.queue.len(), 3);
    assert_eq!(app.queue.current().expect("current").id, 1);
    assert_eq!(app.status.as_deref(), Some("queued Song 2"));
}

#[test]
fn help_toggles_and_clears_any_status_message() {
    let mut app = app();
    app.status = Some("saved something".into());
    app.toggle_help();
    assert!(app.show_help);
    assert!(app.status.is_none());
    app.toggle_help();
    assert!(!app.show_help);
}

#[test]
fn an_empty_search_is_not_submitted() {
    let mut app = app();
    app.search_input = "   ".into();
    assert!(app.submit_search().is_empty());
}

#[test]
fn submitting_a_search_switches_to_the_search_tab() {
    let mut app = app();
    app.focus = Focus::SearchInput;
    app.search_input = " daft punk ".into();
    let effects = app.submit_search();
    assert_eq!(effects, vec![Effect::Search("daft punk".into())]);
    assert_eq!(app.tab, Tab::Search);
    assert_eq!(app.focus, Focus::Browsing);
    assert!(app.search_pending);
}

#[test]
fn search_results_clear_the_pending_flag() {
    let mut app = app();
    app.search_pending = true;
    app.apply(Message::SearchResults(Box::new(results_with_everything())));
    assert!(!app.search_pending);
    assert!(!app.search_results.is_empty());
}

#[test]
fn signing_out_forgets_everything_personal() {
    let mut app = app();
    app.library.set_favorite_track(&track(1), true, clock());
    app.queue.replace(vec![track(1)], 0, 1);
    app.now.track = Some(track(1));
    app.apply(Message::SignedOut);
    assert!(app.library.favorite_tracks().is_empty());
    assert!(app.queue.is_empty());
    assert!(app.now.track.is_none());
    assert_eq!(app.focus, Focus::Login);
}

#[test]
fn a_failed_sign_in_clears_the_password_and_stays_on_the_form() {
    let mut app = app();
    app.login.email = "a@b.co".into();
    app.login.password = "wrong".into();
    app.login.submitting = true;
    app.apply(Message::SignInFailed("Invalid email or password".into()));
    assert!(app.login.password.is_empty());
    assert_eq!(app.login.email, "a@b.co");
    assert!(!app.login.submitting);
    assert_eq!(app.focus, Focus::Login);
}

#[test]
fn needing_verification_moves_focus_to_the_prompt() {
    let mut app = app();
    app.apply(Message::NeedsVerification(
        "http://127.0.0.1:1234/?n=x".into(),
    ));
    assert_eq!(app.focus, Focus::Verification);
    assert!(app.verification_url.is_some());
}

#[test]
fn finishing_verification_retries_the_pending_track() {
    let mut app = app();
    app.push(Screen::Album(album(1, vec![track(1)])));
    app.cursor_to_start();
    app.open_selected();
    app.apply(Message::NeedsVerification("http://127.0.0.1/".into()));
    let effects = app.apply(Message::Verified);
    assert!(matches!(effects.first(), Some(Effect::Play(_))));
    assert_eq!(app.focus, Focus::Browsing);
    assert!(app.verification_url.is_none());
}

#[test]
fn a_failed_browser_check_keeps_the_prompt_open_and_explains_itself() {
    let mut app = app();
    app.apply(Message::NeedsVerification("http://127.0.0.1/".into()));
    app.apply(Message::VerificationFailed(
        "turnstile 110200: this gateway's Turnstile key does not accept 127.0.0.1".into(),
    ));
    assert_eq!(app.focus, Focus::Verification);
    assert!(
        app.verification_error
            .as_deref()
            .unwrap()
            .contains("110200")
    );
}

#[test]
fn a_rejected_token_keeps_the_prompt_open_with_an_explanation() {
    let mut app = app();
    app.apply(Message::NeedsVerification("http://localhost:1/".into()));
    app.apply(Message::VerificationFailed(
        "that token was rejected".into(),
    ));
    assert_eq!(app.focus, Focus::Verification);
    assert!(app.verification_error.is_some());
}

#[test]
fn a_successful_verification_clears_the_prompt_entirely() {
    let mut app = app();
    app.apply(Message::NeedsVerification("http://127.0.0.1/".into()));
    app.apply(Message::VerificationFailed("turnstile 110200".into()));
    app.apply(Message::Verified);
    assert_eq!(app.focus, Focus::Browsing);
    assert!(app.verification_url.is_none());
    assert!(app.verification_error.is_none());
}

#[test]
fn a_remote_sync_does_not_lose_local_favorites() {
    let mut app = app();
    app.library.set_favorite_track(&track(1), true, clock());
    app.apply(Message::Sync(Box::default()));
    assert!(app.library.is_favorite(FavoriteKind::Track, "1"));
}

#[test]
fn pausing_and_resuming_flip_the_player_state() {
    let mut app = app();
    app.now.track = Some(track(1));
    assert_eq!(app.toggle_playback(), vec![Effect::Pause]);
    assert!(app.now.paused);
    assert_eq!(app.toggle_playback(), vec![Effect::Resume]);
    assert!(!app.now.paused);
}

#[test]
fn pressing_play_with_nothing_loaded_starts_the_selection() {
    let mut app = app();
    app.push(Screen::Album(album(1, vec![track(1)])));
    app.cursor_to_start();
    let effects = app.toggle_playback();
    assert!(matches!(effects.first(), Some(Effect::Play(_))));
}

#[test]
fn shuffle_and_repeat_report_their_new_state() {
    let mut app = app();
    app.toggle_shuffle();
    assert_eq!(app.status.as_deref(), Some("shuffle on"));
    app.cycle_repeat();
    assert_eq!(app.status.as_deref(), Some("repeat all"));
}

#[test]
fn the_queue_tab_shows_what_is_queued() {
    let mut app = app();
    app.queue.replace(vec![track(1), track(2)], 0, 1);
    app.switch_tab(Tab::Queue);
    assert_eq!(app.rows().len(), 2);
}

#[test]
fn tracks_without_a_stored_length_are_asked_about_once() {
    let mut app = app();
    for id in 1..=3 {
        let mut saved = track(id);
        saved.duration = 0;
        app.library.set_favorite_track(&saved, true, 1_000 + id);
    }

    let wanted = app.tracks_missing_a_length(24);
    assert_eq!(wanted.len(), 3);
    assert!(
        app.tracks_missing_a_length(24).is_empty(),
        "asking twice for the same track wastes a request"
    );
}

#[test]
fn a_length_that_arrives_later_is_shown_on_the_row() {
    let mut app = app();
    let mut saved = track(7);
    saved.duration = 0;
    app.library.set_favorite_track(&saved, true, 1_000);

    match &app.rows()[0] {
        Row::Track(shown) => assert_eq!(shown.duration, 0),
        other => panic!("expected a track, got {other:?}"),
    }

    let mut detailed = track(7);
    detailed.duration = 213;
    app.apply(Message::TrackDetails(vec![detailed]));

    match &app.rows()[0] {
        Row::Track(shown) => assert_eq!(shown.duration, 213),
        other => panic!("expected a track, got {other:?}"),
    }
}

#[test]
fn a_length_the_catalog_does_not_know_is_not_cached() {
    let mut app = app();
    let mut saved = track(9);
    saved.duration = 0;
    app.library.set_favorite_track(&saved, true, 1_000);

    let mut empty = track(9);
    empty.duration = 0;
    app.apply(Message::TrackDetails(vec![empty]));

    match &app.rows()[0] {
        Row::Track(shown) => assert_eq!(shown.duration, 0),
        other => panic!("expected a track, got {other:?}"),
    }
}

#[test]
fn only_a_bounded_number_of_lengths_is_requested_at_a_time() {
    let mut app = app();
    for id in 1..=40 {
        let mut saved = track(id);
        saved.duration = 0;
        app.library.set_favorite_track(&saved, true, 1_000 + id);
    }
    assert_eq!(app.tracks_missing_a_length(24).len(), 24);
    assert_eq!(app.tracks_missing_a_length(24).len(), 16);
}

#[test]
fn recent_reflects_the_history() {
    let mut app = app();
    app.library.record_play(&track(5), clock());
    app.switch_tab(Tab::Recent);
    assert!(matches!(app.rows()[0], Row::Track(ref t) if t.id == 5));
}
