//! Keybinding table.
//!
//! Every action in the TUI must be achievable with either vim-style keys
//! (`j/k`, `h/l`, `q`) or plain-English equivalents (arrows, tab, escape,
//! `Ctrl+C`). The table is implemented as [`map_key`] — a pure function
//! from [`crossterm::event::KeyEvent`] to [`super::event::AppEvent`].
//!
//! Keeping the mapping in a single pure function makes it easy to unit-test
//! and easy to re-render in a future "press `?` for help" overlay.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::event::AppEvent;
use super::filter::StatusClass;

/// Translate a keyboard event into an [`AppEvent`].
///
/// Three modal flags gate the decision:
///
/// - `input_mode`: URL filter input is focused. Printable keys append to
///   the draft; most global keys are suppressed.
/// - `confirm_clear_mode`: clear-confirmation modal is up; only `y`/`n`/`Esc`
///   fire.
/// - `full_view_mode`: detail-pane full-view is visible; `Esc`/`F` close
///   it; other keys are passed through.
///
/// Returns `None` for keys we do not handle (function keys, unknown
/// modifier combinations, etc.). The caller is expected to treat `None`
/// the same as `AppEvent::NoOp`.
#[must_use]
pub fn map_key(
    key: KeyEvent,
    input_mode: bool,
    confirm_clear_mode: bool,
    full_view_mode: bool,
) -> Option<AppEvent> {
    // Ctrl+C is always quit, regardless of every other mode.
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
    {
        return Some(AppEvent::Quit);
    }

    // Clear-confirmation modal captures y/n/Esc first — nothing else fires
    // while it is up.
    if confirm_clear_mode {
        return match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => Some(AppEvent::ConfirmClear),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                Some(AppEvent::CancelClearConfirm)
            }
            _ => None,
        };
    }

    // URL-filter input mode captures printable keys verbatim; Enter
    // commits, Esc cancels.
    if input_mode {
        return match key.code {
            KeyCode::Enter => Some(AppEvent::CommitFilterInput),
            KeyCode::Esc => Some(AppEvent::CancelFilterInput),
            KeyCode::Backspace => Some(AppEvent::FilterBackspace),
            KeyCode::Char(c) => Some(AppEvent::FilterChar(c)),
            _ => None,
        };
    }

    // Full-view modal: Esc or F closes it; scroll keys pass through.
    if full_view_mode {
        if matches!(key.code, KeyCode::Esc) {
            return Some(AppEvent::CloseFullView);
        }
        if matches!(key.code, KeyCode::Char('f') | KeyCode::Char('F')) {
            return Some(AppEvent::CloseFullView);
        }
        // Fall through to the global table for scroll keys.
    }

    // Alt+1..4 toggle status-class chips. Check before the bare `1..4`
    // fall-through below.
    if key.modifiers.contains(KeyModifiers::ALT) {
        if let KeyCode::Char(c) = key.code {
            return match c {
                '1' => Some(AppEvent::ToggleStatus(StatusClass::Success)),
                '2' => Some(AppEvent::ToggleStatus(StatusClass::Redirect)),
                '3' => Some(AppEvent::ToggleStatus(StatusClass::ClientError)),
                '4' => Some(AppEvent::ToggleStatus(StatusClass::ServerError)),
                _ => None,
            };
        }
    }

    match key.code {
        // Quit — note that Esc here is "quit" only when no modal is open.
        KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => Some(AppEvent::Quit),

        // Vertical list navigation.
        KeyCode::Char('j') | KeyCode::Down => Some(AppEvent::ScrollDown),
        KeyCode::Char('k') | KeyCode::Up => Some(AppEvent::ScrollUp),

        // Detail pane scroll (vim J/K fallthrough to PgUp/PgDn).
        KeyCode::PageDown | KeyCode::Char('J') => Some(AppEvent::DetailScrollDown),
        KeyCode::PageUp | KeyCode::Char('K') => Some(AppEvent::DetailScrollUp),

        // Pane focus.
        KeyCode::Tab | KeyCode::Char('l') | KeyCode::Right => Some(AppEvent::NextPane),
        KeyCode::BackTab | KeyCode::Char('h') | KeyCode::Left => Some(AppEvent::PrevPane),

        // TASK-202
        KeyCode::Char('z') | KeyCode::Char('Z') => Some(AppEvent::ToggleCollapse),
        KeyCode::Char('f') => Some(AppEvent::OpenFullView),
        KeyCode::Char('F') => Some(AppEvent::OpenFullView),
        KeyCode::Char('y') => Some(AppEvent::CopyCurl),
        KeyCode::Char('Y') => Some(AppEvent::CopyCurlRaw),

        // TASK-203
        KeyCode::Char('/') => Some(AppEvent::BeginFilterInput),
        KeyCode::Char('p') | KeyCode::Char('P') => Some(AppEvent::TogglePause),
        KeyCode::Char('c') | KeyCode::Char('C') => Some(AppEvent::BeginClearConfirm),
        KeyCode::Char('1') => Some(AppEvent::ToggleMethod("GET")),
        KeyCode::Char('2') => Some(AppEvent::ToggleMethod("POST")),
        KeyCode::Char('3') => Some(AppEvent::ToggleMethod("PUT")),
        KeyCode::Char('4') => Some(AppEvent::ToggleMethod("DELETE")),
        KeyCode::Char('5') => Some(AppEvent::ToggleMethod("PATCH")),

        // Toggle native mouse behaviour (text selection) vs rustotron's
        // hit-testing. Uppercase `M` so lowercase `m` is free for a
        // future menu.
        KeyCode::Char('M') => Some(AppEvent::ToggleMouseCapture),

        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn vim_down_is_scroll_down() {
        assert_eq!(
            map_key(k(KeyCode::Char('j')), false, false, false),
            Some(AppEvent::ScrollDown)
        );
    }

    #[test]
    fn arrow_down_is_scroll_down() {
        assert_eq!(
            map_key(k(KeyCode::Down), false, false, false),
            Some(AppEvent::ScrollDown)
        );
    }

    #[test]
    fn q_quits() {
        assert_eq!(
            map_key(k(KeyCode::Char('q')), false, false, false),
            Some(AppEvent::Quit)
        );
    }

    #[test]
    fn ctrl_c_quits() {
        let ev = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(map_key(ev, false, false, false), Some(AppEvent::Quit));
    }

    #[test]
    fn tab_switches_pane() {
        assert_eq!(
            map_key(k(KeyCode::Tab), false, false, false),
            Some(AppEvent::NextPane)
        );
        assert_eq!(
            map_key(k(KeyCode::BackTab), false, false, false),
            Some(AppEvent::PrevPane)
        );
    }

    #[test]
    fn page_down_scrolls_detail() {
        assert_eq!(
            map_key(k(KeyCode::PageDown), false, false, false),
            Some(AppEvent::DetailScrollDown)
        );
    }

    #[test]
    fn unknown_key_returns_none() {
        assert_eq!(map_key(k(KeyCode::F(5)), false, false, false), None);
    }

    #[test]
    fn esc_quits_when_no_modal_open() {
        assert_eq!(
            map_key(k(KeyCode::Esc), false, false, false),
            Some(AppEvent::Quit)
        );
    }

    #[test]
    fn hl_navigates_panes() {
        assert_eq!(
            map_key(k(KeyCode::Char('l')), false, false, false),
            Some(AppEvent::NextPane)
        );
        assert_eq!(
            map_key(k(KeyCode::Char('h')), false, false, false),
            Some(AppEvent::PrevPane)
        );
    }

    #[test]
    fn z_toggles_collapse() {
        assert_eq!(
            map_key(k(KeyCode::Char('z')), false, false, false),
            Some(AppEvent::ToggleCollapse)
        );
    }

    #[test]
    fn uppercase_f_opens_full_view_and_lowercase_y_copies_redacted() {
        assert_eq!(
            map_key(k(KeyCode::Char('F')), false, false, false),
            Some(AppEvent::OpenFullView)
        );
        assert_eq!(
            map_key(k(KeyCode::Char('y')), false, false, false),
            Some(AppEvent::CopyCurl)
        );
        assert_eq!(
            map_key(k(KeyCode::Char('Y')), false, false, false),
            Some(AppEvent::CopyCurlRaw)
        );
    }

    #[test]
    fn slash_begins_filter_input() {
        assert_eq!(
            map_key(k(KeyCode::Char('/')), false, false, false),
            Some(AppEvent::BeginFilterInput)
        );
    }

    #[test]
    fn in_input_mode_printable_keys_become_filter_chars() {
        assert_eq!(
            map_key(k(KeyCode::Char('x')), true, false, false),
            Some(AppEvent::FilterChar('x'))
        );
        assert_eq!(
            map_key(k(KeyCode::Enter), true, false, false),
            Some(AppEvent::CommitFilterInput)
        );
        assert_eq!(
            map_key(k(KeyCode::Esc), true, false, false),
            Some(AppEvent::CancelFilterInput)
        );
        assert_eq!(
            map_key(k(KeyCode::Backspace), true, false, false),
            Some(AppEvent::FilterBackspace)
        );
    }

    #[test]
    fn clear_confirm_mode_only_accepts_y_n_esc() {
        assert_eq!(
            map_key(k(KeyCode::Char('y')), false, true, false),
            Some(AppEvent::ConfirmClear)
        );
        assert_eq!(
            map_key(k(KeyCode::Char('n')), false, true, false),
            Some(AppEvent::CancelClearConfirm)
        );
        assert_eq!(
            map_key(k(KeyCode::Esc), false, true, false),
            Some(AppEvent::CancelClearConfirm)
        );
        // Unrelated keys are swallowed.
        assert_eq!(map_key(k(KeyCode::Char('j')), false, true, false), None);
    }

    #[test]
    fn full_view_esc_or_f_closes() {
        assert_eq!(
            map_key(k(KeyCode::Esc), false, false, true),
            Some(AppEvent::CloseFullView)
        );
        assert_eq!(
            map_key(k(KeyCode::Char('F')), false, false, true),
            Some(AppEvent::CloseFullView)
        );
    }

    #[test]
    fn digit_keys_toggle_method_chips() {
        assert_eq!(
            map_key(k(KeyCode::Char('1')), false, false, false),
            Some(AppEvent::ToggleMethod("GET"))
        );
        assert_eq!(
            map_key(k(KeyCode::Char('2')), false, false, false),
            Some(AppEvent::ToggleMethod("POST"))
        );
        assert_eq!(
            map_key(k(KeyCode::Char('5')), false, false, false),
            Some(AppEvent::ToggleMethod("PATCH"))
        );
    }

    #[test]
    fn alt_digit_keys_toggle_status_chips() {
        let ev = KeyEvent::new(KeyCode::Char('1'), KeyModifiers::ALT);
        assert_eq!(
            map_key(ev, false, false, false),
            Some(AppEvent::ToggleStatus(StatusClass::Success))
        );
        let ev = KeyEvent::new(KeyCode::Char('4'), KeyModifiers::ALT);
        assert_eq!(
            map_key(ev, false, false, false),
            Some(AppEvent::ToggleStatus(StatusClass::ServerError))
        );
    }

    #[test]
    fn p_toggles_pause_c_begins_clear() {
        assert_eq!(
            map_key(k(KeyCode::Char('p')), false, false, false),
            Some(AppEvent::TogglePause)
        );
        assert_eq!(
            map_key(k(KeyCode::Char('c')), false, false, false),
            Some(AppEvent::BeginClearConfirm)
        );
    }
}
