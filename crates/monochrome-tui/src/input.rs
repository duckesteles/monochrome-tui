use crate::app::Focus;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Quit,
    Up,
    Down,
    HalfPageUp,
    HalfPageDown,
    Top,
    Bottom,
    Open,
    Back,
    NextTab,
    PreviousTab,
    SelectTab(usize),
    NextSection,
    PreviousSection,
    TogglePlayback,
    SeekForward,
    SeekBackward,
    NextTrack,
    PreviousTrack,
    VolumeUp,
    VolumeDown,
    ToggleMute,
    ToggleShuffle,
    CycleRepeat,
    FocusSearch,
    ToggleFavorite,
    Enqueue,
    OpenQueue,
    ToggleHelp,
    Insert(char),
    Backspace,
    Submit,
    Cancel,
    NextField,
    OpenBrowser,
    None,
}

pub fn resolve(key: KeyEvent, focus: Focus) -> Action {
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
    {
        return Action::Quit;
    }

    match focus {
        Focus::SearchInput | Focus::Login | Focus::Verification => text_entry(key, focus),
        Focus::Browsing => browsing(key),
    }
}

fn text_entry(key: KeyEvent, focus: Focus) -> Action {
    match key.code {
        KeyCode::Enter => Action::Submit,
        KeyCode::Esc => Action::Cancel,
        KeyCode::Backspace => Action::Backspace,
        KeyCode::Tab | KeyCode::Down | KeyCode::Up if focus == Focus::Login => Action::NextField,
        KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            Action::Insert(character)
        }
        _ => Action::None,
    }
}

fn browsing(key: KeyEvent) -> Action {
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let control = key.modifiers.contains(KeyModifiers::CONTROL);

    match key.code {
        KeyCode::Char('d') if control => Action::HalfPageDown,
        KeyCode::Char('u') if control => Action::HalfPageUp,
        KeyCode::Char('j') | KeyCode::Down => Action::Down,
        KeyCode::Char('k') | KeyCode::Up => Action::Up,
        KeyCode::Char('g') => Action::Top,
        KeyCode::Char('G') => Action::Bottom,
        KeyCode::Enter => Action::Open,
        KeyCode::Esc => Action::Back,
        KeyCode::Tab => Action::NextTab,
        KeyCode::BackTab => Action::PreviousTab,
        KeyCode::Char(digit @ '1'..='5') => {
            Action::SelectTab(digit.to_digit(10).unwrap_or(1) as usize - 1)
        }
        KeyCode::Char('l') => Action::NextSection,
        KeyCode::Char('h') => Action::PreviousSection,
        KeyCode::Char(' ') => Action::TogglePlayback,
        KeyCode::Right if shift => Action::NextTrack,
        KeyCode::Left if shift => Action::PreviousTrack,
        KeyCode::Right => Action::SeekForward,
        KeyCode::Left => Action::SeekBackward,
        KeyCode::Char('+') | KeyCode::Char('=') => Action::VolumeUp,
        KeyCode::Char('-') | KeyCode::Char('_') => Action::VolumeDown,
        KeyCode::Char('m') => Action::ToggleMute,
        KeyCode::Char('s') => Action::ToggleShuffle,
        KeyCode::Char('r') => Action::CycleRepeat,
        KeyCode::Char('/') => Action::FocusSearch,
        KeyCode::Char('?') => Action::ToggleHelp,
        KeyCode::Char('f') => Action::ToggleFavorite,
        KeyCode::Char('a') => Action::Enqueue,
        KeyCode::Char('q') => Action::OpenQueue,
        KeyCode::Char('Q') => Action::Quit,
        _ => Action::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn with(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn ctrl_c_always_quits_whatever_has_focus() {
        for focus in [
            Focus::Browsing,
            Focus::SearchInput,
            Focus::Login,
            Focus::Verification,
        ] {
            assert_eq!(
                resolve(with(KeyCode::Char('c'), KeyModifiers::CONTROL), focus),
                Action::Quit
            );
        }
    }

    #[test]
    fn lowercase_q_opens_the_queue_and_uppercase_q_quits() {
        assert_eq!(
            resolve(key(KeyCode::Char('q')), Focus::Browsing),
            Action::OpenQueue
        );
        assert_eq!(
            resolve(key(KeyCode::Char('Q')), Focus::Browsing),
            Action::Quit
        );
    }

    #[test]
    fn arrows_navigate_the_list_and_shift_arrows_change_track() {
        assert_eq!(resolve(key(KeyCode::Down), Focus::Browsing), Action::Down);
        assert_eq!(resolve(key(KeyCode::Up), Focus::Browsing), Action::Up);
        assert_eq!(
            resolve(with(KeyCode::Right, KeyModifiers::SHIFT), Focus::Browsing),
            Action::NextTrack
        );
        assert_eq!(
            resolve(with(KeyCode::Left, KeyModifiers::SHIFT), Focus::Browsing),
            Action::PreviousTrack
        );
    }

    #[test]
    fn bare_arrows_seek_matching_the_web_client() {
        assert_eq!(
            resolve(key(KeyCode::Right), Focus::Browsing),
            Action::SeekForward
        );
        assert_eq!(
            resolve(key(KeyCode::Left), Focus::Browsing),
            Action::SeekBackward
        );
    }

    #[test]
    fn vim_keys_mirror_the_arrows() {
        assert_eq!(
            resolve(key(KeyCode::Char('j')), Focus::Browsing),
            Action::Down
        );
        assert_eq!(
            resolve(key(KeyCode::Char('k')), Focus::Browsing),
            Action::Up
        );
    }

    #[test]
    fn volume_is_on_plus_and_minus_because_the_arrows_are_taken() {
        assert_eq!(
            resolve(key(KeyCode::Char('+')), Focus::Browsing),
            Action::VolumeUp
        );
        assert_eq!(
            resolve(key(KeyCode::Char('-')), Focus::Browsing),
            Action::VolumeDown
        );
    }

    #[test]
    fn digits_select_tabs_directly() {
        assert_eq!(
            resolve(key(KeyCode::Char('1')), Focus::Browsing),
            Action::SelectTab(0)
        );
        assert_eq!(
            resolve(key(KeyCode::Char('5')), Focus::Browsing),
            Action::SelectTab(4)
        );
        assert_eq!(
            resolve(key(KeyCode::Char('6')), Focus::Browsing),
            Action::None
        );
    }

    #[test]
    fn half_page_movement_needs_control() {
        assert_eq!(
            resolve(
                with(KeyCode::Char('d'), KeyModifiers::CONTROL),
                Focus::Browsing
            ),
            Action::HalfPageDown
        );
        assert_eq!(
            resolve(
                with(KeyCode::Char('u'), KeyModifiers::CONTROL),
                Focus::Browsing
            ),
            Action::HalfPageUp
        );
    }

    #[test]
    fn typing_in_a_field_inserts_characters_rather_than_triggering_commands() {
        assert_eq!(
            resolve(key(KeyCode::Char('q')), Focus::SearchInput),
            Action::Insert('q')
        );
        assert_eq!(
            resolve(key(KeyCode::Char(' ')), Focus::SearchInput),
            Action::Insert(' ')
        );
        assert_eq!(
            resolve(key(KeyCode::Char('/')), Focus::Login),
            Action::Insert('/')
        );
    }

    #[test]
    fn escape_leaves_a_field_and_enter_submits_it() {
        assert_eq!(
            resolve(key(KeyCode::Esc), Focus::SearchInput),
            Action::Cancel
        );
        assert_eq!(resolve(key(KeyCode::Enter), Focus::Login), Action::Submit);
    }

    #[test]
    fn tab_moves_between_login_fields_but_switches_tabs_while_browsing() {
        assert_eq!(resolve(key(KeyCode::Tab), Focus::Login), Action::NextField);
        assert_eq!(resolve(key(KeyCode::Tab), Focus::Browsing), Action::NextTab);
    }

    #[test]
    fn a_token_can_be_pasted_into_the_verification_prompt() {
        assert_eq!(
            resolve(key(KeyCode::Char('e')), Focus::Verification),
            Action::Insert('e')
        );
        assert_eq!(
            resolve(key(KeyCode::Char('.')), Focus::Verification),
            Action::Insert('.')
        );
        assert_eq!(
            resolve(key(KeyCode::Backspace), Focus::Verification),
            Action::Backspace
        );
        assert_eq!(
            resolve(key(KeyCode::Enter), Focus::Verification),
            Action::Submit
        );
        assert_eq!(
            resolve(key(KeyCode::Esc), Focus::Verification),
            Action::Cancel
        );
    }

    #[test]
    fn question_mark_opens_the_shortcut_list() {
        assert_eq!(
            resolve(key(KeyCode::Char('?')), Focus::Browsing),
            Action::ToggleHelp
        );
    }

    #[test]
    fn escape_goes_back_while_browsing() {
        assert_eq!(resolve(key(KeyCode::Esc), Focus::Browsing), Action::Back);
    }
}
