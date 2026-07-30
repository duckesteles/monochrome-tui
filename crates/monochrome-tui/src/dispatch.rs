use crate::app::{App, Effect, Focus, LoginField};
use crate::input::{self, Action};
use crossterm::event::KeyEvent;

const SEEK_STEP: f64 = 10.0;
const VOLUME_STEP: f32 = 0.05;

pub fn on_key(app: &mut App, key: KeyEvent) -> Vec<Effect> {
    let action = input::resolve(key, app.focus);

    if app.show_help {
        match action {
            Action::Quit => return vec![Effect::Quit],
            Action::Down => app.scroll_help(1),
            Action::Up => app.scroll_help(-1),
            Action::HalfPageDown => app.scroll_help(8),
            Action::HalfPageUp => app.scroll_help(-8),
            Action::Top => app.help_scroll = 0,
            Action::Bottom => app.scroll_help(i16::MAX),
            Action::Back | Action::ToggleHelp | Action::OpenQueue => app.toggle_help(),
            _ => {}
        }
        return Vec::new();
    }

    if !matches!(action, Action::None) {
        app.status = None;
    }

    match action {
        Action::Quit => vec![Effect::Quit],
        Action::Up => {
            app.move_cursor(-1);
            Vec::new()
        }
        Action::Down => {
            app.move_cursor(1);
            Vec::new()
        }
        Action::HalfPageUp => {
            app.move_cursor(-8);
            Vec::new()
        }
        Action::HalfPageDown => {
            app.move_cursor(8);
            Vec::new()
        }
        Action::Top => {
            app.cursor_to_start();
            Vec::new()
        }
        Action::Bottom => {
            app.cursor_to_end();
            Vec::new()
        }
        Action::Open => app.open_selected(),
        Action::Back => {
            app.pop();
            Vec::new()
        }
        Action::NextTab => {
            app.next_tab(true);
            Vec::new()
        }
        Action::PreviousTab => {
            app.next_tab(false);
            Vec::new()
        }
        Action::SelectTab(index) => {
            if let Some(tab) = crate::app::Tab::ALL.get(index) {
                app.switch_tab(*tab);
            }
            Vec::new()
        }
        Action::NextSection => {
            app.cycle_section(true);
            Vec::new()
        }
        Action::PreviousSection => {
            app.cycle_section(false);
            Vec::new()
        }
        Action::OpenQueue => {
            app.switch_tab(crate::app::Tab::Queue);
            Vec::new()
        }
        Action::TogglePlayback => app.toggle_playback(),
        Action::SeekForward => app.seek_by(SEEK_STEP),
        Action::SeekBackward => app.seek_by(-SEEK_STEP),
        Action::NextTrack => app.play_next(true),
        Action::PreviousTrack => app.play_previous(),
        Action::VolumeUp => app.change_volume(VOLUME_STEP),
        Action::VolumeDown => app.change_volume(-VOLUME_STEP),
        Action::ToggleMute => app.toggle_mute(),
        Action::ToggleShuffle => {
            app.toggle_shuffle();
            Vec::new()
        }
        Action::CycleRepeat => {
            app.cycle_repeat();
            Vec::new()
        }
        Action::ToggleHelp => {
            app.toggle_help();
            Vec::new()
        }
        Action::FocusSearch => {
            app.focus = Focus::SearchInput;
            Vec::new()
        }
        Action::ToggleFavorite => app.toggle_favorite(),
        Action::Enqueue => {
            app.queue_selected();
            Vec::new()
        }
        Action::Insert(character) => {
            match app.focus {
                Focus::SearchInput => app.search_input.push(character),
                Focus::Login => match app.login.field {
                    LoginField::Email => app.login.email.push(character),
                    LoginField::Password => app.login.password.push(character),
                },
                Focus::Verification => app.verification_input.push(character),
                _ => {}
            }
            Vec::new()
        }
        Action::Backspace => {
            match app.focus {
                Focus::SearchInput => {
                    app.search_input.pop();
                }
                Focus::Login => match app.login.field {
                    LoginField::Email => {
                        app.login.email.pop();
                    }
                    LoginField::Password => {
                        app.login.password.pop();
                    }
                },
                Focus::Verification => {
                    app.verification_input.pop();
                }
                _ => {}
            }
            Vec::new()
        }
        Action::NextField => {
            app.login.field = match app.login.field {
                LoginField::Email => LoginField::Password,
                LoginField::Password => LoginField::Email,
            };
            Vec::new()
        }
        Action::Submit => match app.focus {
            Focus::Verification => {
                if app.verification_input.trim().is_empty() {
                    vec![Effect::OpenBrowser]
                } else {
                    app.submit_verification()
                }
            }
            Focus::SearchInput => app.submit_search(),
            Focus::Login => {
                if app.login.email.trim().is_empty() || app.login.password.is_empty() {
                    app.status = Some("enter your email and password".into());
                    Vec::new()
                } else {
                    app.login.submitting = true;
                    vec![Effect::SignIn {
                        email: app.login.email.trim().to_string(),
                        password: std::mem::take(&mut app.login.password),
                    }]
                }
            }
            _ => Vec::new(),
        },
        Action::Cancel => {
            match app.focus {
                Focus::SearchInput | Focus::Verification => app.focus = Focus::Browsing,
                _ => {}
            }
            Vec::new()
        }
        Action::OpenBrowser => vec![Effect::OpenBrowser],
        Action::None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Message, Screen, Tab};
    use crossterm::event::{KeyCode, KeyModifiers};
    use monochrome_core::model::Quality;

    fn app() -> App {
        App::with_clock(Quality::Lossless, 0.5, || 1_700_000_000_000)
    }

    fn press(app: &mut App, code: KeyCode) -> Vec<Effect> {
        on_key(app, KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn press_with(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Vec<Effect> {
        on_key(app, KeyEvent::new(code, modifiers))
    }

    #[test]
    fn typing_reaches_the_search_field_and_enter_submits_it() {
        let mut app = app();
        press(&mut app, KeyCode::Char('/'));
        assert_eq!(app.focus, Focus::SearchInput);
        for character in "daft".chars() {
            press(&mut app, KeyCode::Char(character));
        }
        assert_eq!(app.search_input, "daft");
        press(&mut app, KeyCode::Backspace);
        assert_eq!(app.search_input, "daf");
        let effects = press(&mut app, KeyCode::Enter);
        assert_eq!(effects, vec![Effect::Search("daf".into())]);
    }

    #[test]
    fn escaping_the_search_field_returns_to_the_list() {
        let mut app = app();
        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.focus, Focus::Browsing);
    }

    #[test]
    fn typing_reaches_the_right_login_field_and_tab_switches_them() {
        let mut app = app();
        app.focus = Focus::Login;
        for character in "me@x.co".chars() {
            press(&mut app, KeyCode::Char(character));
        }
        assert_eq!(app.login.email, "me@x.co");

        press(&mut app, KeyCode::Tab);
        assert_eq!(app.login.field, LoginField::Password);
        for character in "secret".chars() {
            press(&mut app, KeyCode::Char(character));
        }
        assert_eq!(app.login.password, "secret");
        assert_eq!(app.login.email, "me@x.co");
    }

    #[test]
    fn signing_in_hands_over_the_password_and_leaves_none_behind() {
        let mut app = app();
        app.focus = Focus::Login;
        app.login.email = " me@x.co ".into();
        app.login.password = "secret".into();

        let effects = press(&mut app, KeyCode::Enter);
        assert_eq!(
            effects,
            vec![Effect::SignIn {
                email: "me@x.co".into(),
                password: "secret".into()
            }]
        );
        assert!(app.login.password.is_empty());
        assert!(app.login.submitting);
    }

    #[test]
    fn an_incomplete_login_is_refused_without_a_request() {
        let mut app = app();
        app.focus = Focus::Login;
        app.login.email = "me@x.co".into();
        assert!(press(&mut app, KeyCode::Enter).is_empty());
        assert!(!app.login.submitting);
        assert!(app.status.is_some());
    }

    #[test]
    fn the_help_overlay_opens_on_question_mark_and_closes_on_escape() {
        let mut app = app();
        press(&mut app, KeyCode::Char('?'));
        assert!(app.show_help);
        press(&mut app, KeyCode::Esc);
        assert!(!app.show_help);
    }

    #[test]
    fn question_mark_closes_the_help_it_opened() {
        let mut app = app();
        press(&mut app, KeyCode::Char('?'));
        press(&mut app, KeyCode::Char('?'));
        assert!(!app.show_help);
    }

    #[test]
    fn the_help_scrolls_instead_of_closing() {
        let mut app = app();
        press(&mut app, KeyCode::Char('?'));
        press(&mut app, KeyCode::Down);
        assert!(app.show_help, "moving must not dismiss the help");
        assert_eq!(app.help_scroll, 1);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.help_scroll, 2);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.help_scroll, 1);
    }

    #[test]
    fn the_help_cannot_be_scrolled_above_its_first_line() {
        let mut app = app();
        press(&mut app, KeyCode::Char('?'));
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.help_scroll, 0);
    }

    #[test]
    fn the_help_stops_at_its_last_line() {
        let mut app = app();
        press(&mut app, KeyCode::Char('?'));
        press(&mut app, KeyCode::Char('G'));
        let furthest = app.help_scroll;
        press(&mut app, KeyCode::Char('G'));
        assert_eq!(app.help_scroll, furthest);
        assert!(furthest > 0);
    }

    #[test]
    fn a_key_with_no_meaning_in_the_help_leaves_it_open() {
        let mut app = app();
        app.switch_tab(Tab::Library);
        press(&mut app, KeyCode::Char('?'));
        let effects = press(&mut app, KeyCode::Char('2'));
        assert!(effects.is_empty());
        assert!(app.show_help);
        assert_eq!(app.tab, Tab::Library);
    }

    #[test]
    fn quitting_still_works_while_the_help_is_open() {
        let mut app = app();
        press(&mut app, KeyCode::Char('?'));
        assert_eq!(press(&mut app, KeyCode::Char('Q')), vec![Effect::Quit]);
    }

    #[test]
    fn ctrl_c_quits_from_a_text_field() {
        let mut app = app();
        app.focus = Focus::Login;
        let effects = press_with(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(effects, vec![Effect::Quit]);
    }

    #[test]
    fn seeking_moves_by_ten_seconds_in_both_directions() {
        let mut app = app();
        app.now.duration = Some(200.0);
        app.now.position = 50.0;
        app.now.track = Some(monochrome_core::model::Track {
            id: 1,
            title: "x".into(),
            duration: 200,
            explicit: false,
            artist: None,
            artists: Vec::new(),
            album: None,
            isrc: None,
            track_number: None,
            volume_number: None,
            copyright: None,
            version: None,
            quality: Quality::Lossless,
            replay_gain: None,
            peak: None,
            stream_ready: true,
        });

        assert_eq!(press(&mut app, KeyCode::Right), vec![Effect::Seek(60.0)]);
        assert_eq!(press(&mut app, KeyCode::Left), vec![Effect::Seek(50.0)]);
    }

    #[test]
    fn muting_and_unmuting_round_trips_through_the_key_map() {
        let mut app = app();
        let before = app.volume;
        assert_eq!(
            press(&mut app, KeyCode::Char('m')),
            vec![Effect::Volume(0.0)]
        );
        assert_eq!(
            press(&mut app, KeyCode::Char('m')),
            vec![Effect::Volume(before)]
        );
    }

    #[test]
    fn digits_and_tab_move_between_tabs() {
        let mut app = app();
        press(&mut app, KeyCode::Char('3'));
        assert_eq!(app.tab, Tab::Playlists);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.tab, Tab::Recent);
        press(&mut app, KeyCode::Char('q'));
        assert_eq!(app.tab, Tab::Queue);
    }

    #[test]
    fn a_pasted_verification_token_is_submitted_and_an_empty_one_opens_the_browser() {
        let mut app = app();
        app.apply(Message::NeedsVerification("http://localhost:1/".into()));
        assert_eq!(press(&mut app, KeyCode::Enter), vec![Effect::OpenBrowser]);

        for character in "abc".chars() {
            press(&mut app, KeyCode::Char(character));
        }
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            vec![Effect::UseToken("abc".into())]
        );
    }

    #[test]
    fn escape_walks_back_out_of_a_detail_screen() {
        let mut app = app();
        app.push(Screen::Loading("something".into()));
        press(&mut app, KeyCode::Esc);
        assert!(app.stack.is_empty());
    }

    #[test]
    fn an_unbound_key_changes_nothing() {
        let mut app = app();
        let before = app.tab;
        assert!(press(&mut app, KeyCode::Char('z')).is_empty());
        assert_eq!(app.tab, before);
    }
}
