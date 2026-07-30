use crate::app::*;
use crate::theme::Theme;
use crate::views::{self, fit, format_duration};
use monochrome_core::model::{Album, ArtistRef, Quality, Track};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::widgets::ListState;

fn track(id: u64, title: &str) -> Track {
    Track {
        id,
        title: title.into(),
        duration: 207,
        explicit: false,
        artist: Some(ArtistRef {
            id: 7,
            name: "Daft Punk".into(),
            picture: None,
        }),
        artists: Vec::new(),
        album: None,
        isrc: None,
        track_number: Some(1),
        volume_number: Some(1),
        copyright: None,
        version: None,
        quality: Quality::Lossless,
        replay_gain: None,
        peak: None,
        stream_ready: true,
    }
}

fn album(tracks: Vec<Track>) -> Album {
    Album {
        id: 1,
        title: "Discovery".into(),
        cover: None,
        release_date: Some("2001-03-12".into()),
        artist: Some(ArtistRef {
            id: 7,
            name: "Daft Punk".into(),
            picture: None,
        }),
        number_of_tracks: Some(tracks.len() as u32),
        duration: None,
        explicit: false,
        quality: Quality::Lossless,
        album_type: None,
        copyright: None,
        tracks,
    }
}

fn draw(app: &App, width: u16, height: u16) -> ratatui::buffer::Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let theme = Theme::default();
    let mut state = ListState::default();
    terminal
        .draw(|frame| views::render(frame, app, &theme, &mut state))
        .expect("draw");
    terminal.backend().buffer().clone()
}

fn text(buffer: &ratatui::buffer::Buffer) -> String {
    let area = buffer.area();
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn app() -> App {
    App::with_clock(Quality::Lossless, 0.72, || 1_700_000_000_000)
}

#[test]
fn nothing_on_screen_paints_a_background() {
    let mut app = app();
    app.push(Screen::Album(album(vec![
        track(1, "One More Time"),
        track(2, "Aerodynamic"),
    ])));
    app.now.track = Some(track(1, "One More Time"));
    app.now.position = 30.0;
    app.now.duration = Some(207.0);
    app.status = Some("saved One More Time".into());

    let buffer = draw(&app, 80, 24);
    let area = *buffer.area();
    for y in 0..area.height {
        for x in 0..area.width {
            let cell = &buffer[(x, y)];
            assert_eq!(
                cell.bg,
                ratatui::style::Color::Reset,
                "cell {x},{y} paints a background, which would break transparency"
            );
        }
    }
}

#[test]
fn the_tab_row_lists_every_tab() {
    let rendered = text(&draw(&app(), 80, 24));
    let first = rendered.lines().next().expect("tab row");
    for label in ["library", "search", "playlists", "recent", "queue"] {
        assert!(first.contains(label), "{label} missing from {first:?}");
    }
}

#[test]
fn the_library_sections_show_only_at_the_library_root() {
    let mut app = app();
    let rendered = text(&draw(&app, 80, 24));
    assert!(rendered.lines().next().expect("row").contains("artists"));

    app.switch_tab(Tab::Recent);
    let rendered = text(&draw(&app, 80, 24));
    assert!(!rendered.lines().next().expect("row").contains("artists"));
}

#[test]
fn the_breadcrumb_is_drawn_under_the_tabs() {
    let mut app = app();
    app.push(Screen::Album(album(vec![track(1, "One More Time")])));
    let rendered = text(&draw(&app, 80, 24));
    assert!(
        rendered
            .lines()
            .nth(1)
            .expect("breadcrumb")
            .contains("library > Discovery")
    );
}

#[test]
fn track_rows_show_the_title_artist_and_duration() {
    let mut app = app();
    app.push(Screen::Album(album(vec![track(1, "One More Time")])));
    let rendered = text(&draw(&app, 80, 24));
    assert!(rendered.contains("One More Time"));
    assert!(rendered.contains("Daft Punk"));
    assert!(rendered.contains("3:27"));
}

#[test]
fn a_track_with_no_stored_length_shows_nothing_rather_than_zero() {
    let mut app = app();
    let mut unknown = track(1, "Yazamadim");
    unknown.duration = 0;
    app.push(Screen::Album(album(vec![unknown])));
    let rendered = text(&draw(&app, 80, 24));
    assert!(rendered.contains("Yazamadim"));
    assert!(!rendered.contains("0:00"));
}

#[test]
fn the_now_playing_line_omits_an_unknown_length() {
    let mut app = app();
    let mut unknown = track(1, "Fabrika Kizi");
    unknown.duration = 0;
    app.now.track = Some(unknown);
    app.now.duration = None;
    app.now.position = 12.0;
    let rendered = text(&draw(&app, 80, 24));
    assert!(rendered.contains("0:12"));
    assert!(!rendered.contains("0:12 / "));
}

#[test]
fn the_playing_track_is_marked() {
    let mut app = app();
    app.push(Screen::Album(album(vec![track(1, "One More Time")])));
    app.now.track = Some(track(1, "One More Time"));
    let rendered = text(&draw(&app, 80, 24));
    assert!(rendered.contains("\u{25b6}"));
}

#[test]
fn a_populated_library_lists_its_saved_tracks() {
    let mut app = app();
    for id in 1..=3 {
        app.library
            .set_favorite_track(&track(id, &format!("Saved {id}")), true, 1_000 + id);
    }
    let rendered = text(&draw(&app, 80, 24));
    assert!(rendered.contains("Saved 3"));
    assert!(rendered.contains("Saved 1"));
    assert!(!rendered.contains("no saved tracks yet"));
}

#[test]
fn the_status_line_says_where_the_shortcuts_are() {
    assert!(text(&draw(&app(), 80, 24)).contains("? keys"));
}

#[test]
fn roomy_rows_put_a_blank_line_between_tracks() {
    let mut app = app();
    app.push(Screen::Album(album(vec![
        track(1, "First"),
        track(2, "Second"),
    ])));

    let tight = text(&draw(&app, 80, 24));
    let first_tight = tight
        .lines()
        .position(|l| l.contains("First"))
        .expect("first");
    let second_tight = tight
        .lines()
        .position(|l| l.contains("Second"))
        .expect("second");
    assert_eq!(second_tight - first_tight, 1);

    app.roomy_rows = true;
    let roomy = text(&draw(&app, 80, 24));
    let first = roomy
        .lines()
        .position(|l| l.contains("First"))
        .expect("first");
    let second = roomy
        .lines()
        .position(|l| l.contains("Second"))
        .expect("second");
    assert_eq!(second - first, 2);
}

#[test]
fn the_help_overlay_can_be_scrolled() {
    let mut app = app();
    app.show_help = true;
    let top = text(&draw(&app, 80, 14));
    app.help_scroll = 6;
    let scrolled = text(&draw(&app, 80, 14));
    assert_ne!(top, scrolled, "scrolling should move the help");
}

#[test]
fn the_help_overlay_lists_the_shortcuts_it_documents() {
    let mut app = app();
    app.show_help = true;
    let rendered = text(&draw(&app, 80, 40));
    for keys in ["j k / arrows", "space", "/", "?", "Q ctrl+c"] {
        assert!(rendered.contains(keys), "{keys} missing from the help");
    }
    assert!(rendered.contains("volume"));
    assert!(rendered.contains("save or unsave"));
}

#[test]
fn the_help_overlay_covers_the_list_rather_than_squeezing_it() {
    let mut app = app();
    app.push(Screen::Album(album(vec![track(1, "One More Time")])));
    app.show_help = true;
    let rendered = text(&draw(&app, 80, 40));
    assert!(!rendered.contains("One More Time"));
    assert!(rendered.contains("play or open"));
}

fn highlighted_row(buffer: &ratatui::buffer::Buffer) -> Option<String> {
    let area = *buffer.area();
    for y in 0..area.height {
        let reversed = (0..area.width).any(|x| {
            buffer[(x, y)]
                .modifier
                .contains(ratatui::style::Modifier::REVERSED)
        });
        if reversed {
            let line: String = (0..area.width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect();
            return Some(line.trim().to_string());
        }
    }
    None
}

#[test]
fn the_highlighted_row_is_the_one_that_plays() {
    let mut app = app();
    let listing: Vec<_> = (1..=6)
        .map(|id| track(id, &format!("Track number {id}")))
        .collect();
    app.push(Screen::Album(album(listing)));

    for step in 0..5 {
        app.cursor_to_start();
        app.move_cursor(step);

        let shown = highlighted_row(&draw(&app, 80, 24)).expect("a row is highlighted");
        let selected = match app.selected_row().expect("a row is selected") {
            Row::Track(chosen) => chosen.title,
            other => panic!("expected a track, got {other:?}"),
        };

        assert!(
            shown.contains(&selected),
            "row {step}: the screen highlights {shown:?} but enter would play {selected:?}"
        );
    }
}

#[test]
fn the_highlighted_row_is_the_one_that_plays_when_rows_are_roomy() {
    let mut app = app();
    app.roomy_rows = true;
    let listing: Vec<_> = (1..=6)
        .map(|id| track(id, &format!("Track number {id}")))
        .collect();
    app.push(Screen::Album(album(listing)));

    for step in 0..5 {
        app.cursor_to_start();
        app.move_cursor(step);

        let shown = highlighted_row(&draw(&app, 80, 24)).expect("a row is highlighted");
        let selected = match app.selected_row().expect("a row is selected") {
            Row::Track(chosen) => chosen.title,
            other => panic!("expected a track, got {other:?}"),
        };

        assert!(
            shown.contains(&selected),
            "row {step}: the screen highlights {shown:?} but enter would play {selected:?}"
        );
    }
}

#[test]
fn the_highlighted_row_is_the_one_that_plays_in_a_scrolled_list() {
    let mut app = app();
    let listing: Vec<_> = (1..=60)
        .map(|id| track(id, &format!("Track number {id}")))
        .collect();
    app.push(Screen::Album(album(listing)));
    app.cursor_to_start();
    app.move_cursor(45);

    let shown = highlighted_row(&draw(&app, 80, 24)).expect("a row is highlighted");
    let selected = match app.selected_row().expect("a row is selected") {
        Row::Track(chosen) => chosen.title,
        other => panic!("expected a track, got {other:?}"),
    };
    assert!(
        shown.contains(&selected),
        "the screen highlights {shown:?} but enter would play {selected:?}"
    );
}

#[test]
fn an_empty_screen_explains_itself() {
    let rendered = text(&draw(&app(), 80, 24));
    assert!(rendered.contains("no saved tracks yet"));
}

#[test]
fn the_now_playing_line_shows_position_and_length() {
    let mut app = app();
    app.now.track = Some(track(1, "One More Time"));
    app.now.position = 84.0;
    app.now.duration = Some(207.0);
    let rendered = text(&draw(&app, 80, 24));
    assert!(rendered.contains("1:24 / 3:27"), "{rendered}");
}

#[test]
fn an_idle_player_says_so() {
    assert!(text(&draw(&app(), 80, 24)).contains("nothing playing"));
}

#[test]
fn the_status_line_reports_volume() {
    assert!(text(&draw(&app(), 80, 24)).contains("vol 72%"));
}

#[test]
fn a_status_message_replaces_the_status_line() {
    let mut app = app();
    app.status = Some("every catalog instance failed".into());
    let rendered = text(&draw(&app, 80, 24));
    assert!(rendered.contains("every catalog instance failed"));
    assert!(!rendered.contains("vol 72%"));
}

#[test]
fn the_search_field_shows_what_is_typed() {
    let mut app = app();
    app.focus = Focus::SearchInput;
    app.search_input = "daft punk".into();
    let rendered = text(&draw(&app, 80, 24));
    assert!(rendered.contains("search: daft punk"));
}

#[test]
fn the_login_form_masks_the_password() {
    let mut app = app();
    app.focus = Focus::Login;
    app.login.email = "me@example.com".into();
    app.login.password = "hunter2".into();
    let rendered = text(&draw(&app, 80, 24));
    assert!(rendered.contains("me@example.com"));
    assert!(rendered.contains("*******"));
    assert!(!rendered.contains("hunter2"));
}

#[test]
fn the_verification_prompt_shows_the_local_url() {
    let mut app = app();
    app.focus = Focus::Verification;
    app.verification_url = Some("http://127.0.0.1:41234/?n=abc".into());
    let rendered = text(&draw(&app, 80, 24));
    assert!(rendered.contains("http://127.0.0.1:41234"));
    assert!(rendered.contains("nothing else is needed"));
}

#[test]
fn a_failed_browser_check_says_why_and_keeps_the_tab_address() {
    let mut app = app();
    app.focus = Focus::Verification;
    app.verification_url = Some("http://127.0.0.1:41234/?n=abc".into());
    app.verification_error =
        Some("turnstile 110200: this gateway's Turnstile key does not accept 127.0.0.1".into());
    let rendered = text(&draw(&app, 90, 26));
    assert!(rendered.contains("110200"));
    assert!(rendered.contains("http://127.0.0.1:41234"));
}

#[test]
fn no_screen_ever_asks_anyone_to_open_a_browser_console() {
    let mut app = app();
    app.focus = Focus::Verification;
    app.verification_url = Some("http://127.0.0.1:41234/?n=abc".into());
    app.verification_error = Some("turnstile 110200: rejected".into());
    let rendered = text(&draw(&app, 90, 26));
    assert!(!rendered.contains("localStorage"));
    assert!(!rendered.contains("amazon_turnstile_jwt"));
    assert!(!rendered.contains("console"));
}

#[test]
fn a_pasted_token_is_never_shown_in_full() {
    let mut app = app();
    app.focus = Focus::Verification;
    app.verification_input = "example-token-value-of-some-length".into();
    let rendered = text(&draw(&app, 90, 26));
    assert!(!rendered.contains("secret-payload"));
    assert!(rendered.contains("characters"));
}

#[test]
fn a_tiny_terminal_says_so_instead_of_panicking() {
    let rendered = text(&draw(&app(), 20, 6));
    assert!(rendered.contains("too small"));
}

#[test]
fn a_narrow_terminal_still_renders_every_row() {
    let mut app = app();
    app.push(Screen::Album(album(vec![
        track(1, "A Very Long Track Title That Will Not Fit"),
        track(2, "Aerodynamic"),
    ])));
    let buffer = draw(&app, 40, 20);
    let rendered = text(&buffer);
    assert!(
        rendered.contains("\u{2026}"),
        "long titles should be elided"
    );
    assert_eq!(buffer.area().width, 40);
}

#[test]
fn no_line_ever_overflows_the_terminal_width() {
    let mut app = app();
    app.push(Screen::Album(album(vec![track(
        1,
        "A Very Long Track Title That Would Overflow A Narrow Terminal Window",
    )])));
    for width in [30u16, 48, 80, 120] {
        let buffer = draw(&app, width, 20);
        assert_eq!(buffer.area().width, width);
        for line in text(&buffer).lines() {
            assert_eq!(line.chars().count(), width as usize);
        }
    }
}

#[test]
fn durations_are_formatted_the_way_a_listener_expects() {
    assert_eq!(format_duration(0.0), "0:00");
    assert_eq!(format_duration(9.4), "0:09");
    assert_eq!(format_duration(207.0), "3:27");
    assert_eq!(format_duration(3661.0), "1:01:01");
    assert_eq!(format_duration(-5.0), "0:00");
    assert_eq!(format_duration(f64::NAN), "0:00");
}

#[test]
fn text_is_elided_rather_than_cut_mid_word_at_the_edge() {
    assert_eq!(fit("hello", 10), "hello");
    assert_eq!(fit("hello world", 8), "hello w\u{2026}");
    assert_eq!(fit("hello", 0), "");
}

#[test]
fn wide_characters_are_measured_by_display_width() {
    let elided = fit("日本語のタイトルです", 8);
    assert!(elided.ends_with('\u{2026}'));
    assert!(unicode_width::UnicodeWidthStr::width(elided.as_str()) <= 8);
}
