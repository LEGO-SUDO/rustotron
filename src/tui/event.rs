//! Crossterm `Event` → domain `AppEvent` adapter.
//!
//! The TUI main loop pulls raw [`crossterm::event::Event`] values off the
//! terminal and converts them here into a small, testable [`AppEvent`] enum
//! that the [`super::app::App`] understands.
//!
//! This file is deliberately thin — the actual mappings live in
//! [`super::keys`] (keyboard) and [`super::mouse`] (mouse hit-testing).
//! This module just does the top-level dispatch.

use crossterm::event::Event as CrosstermEvent;

use super::filter::StatusClass;
use super::keys::map_key;
use super::mouse::{HitRegion, dispatch_mouse};

/// Which pane the focus highlight is on.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PaneId {
    /// The left-hand request list.
    List,
    /// The right-hand request detail pane.
    Detail,
}

impl PaneId {
    /// The "next" pane in tab order.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            PaneId::List => PaneId::Detail,
            PaneId::Detail => PaneId::List,
        }
    }

    /// The "previous" pane in tab order. Mirrors [`PaneId::next`] at v1
    /// (only two panes), kept as a method so the call sites stay readable
    /// as the pane count grows.
    #[must_use]
    pub fn prev(self) -> Self {
        self.next()
    }
}

/// Small vocabulary of things the main loop tells `App` to do.
///
/// Mouse clicks resolve to one of these via the [`HitRegion`] table rather
/// than turning into a bespoke variant per button. That's what keeps the
/// view layer pluggable.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum AppEvent {
    /// Move the list selection up by one.
    ScrollUp,
    /// Move the list selection down by one.
    ScrollDown,
    /// Scroll the detail pane up by one line.
    DetailScrollUp,
    /// Scroll the detail pane down by one line.
    DetailScrollDown,
    /// Jump focus to the previous pane.
    PrevPane,
    /// Jump focus to the next pane.
    NextPane,
    /// Focus a specific pane (typically from a mouse click).
    FocusPane(PaneId),
    /// Select a specific row index in the list (typically from a mouse click).
    SelectRow(usize),
    /// Quit the application (maps to `q` or `Ctrl+C`).
    Quit,
    /// Terminal size changed — the next render will re-layout.
    Resize,

    // ── TASK-202 ─────────────────────────────────────────────────────
    /// Toggle collapse state of the section nearest the scroll cursor.
    ToggleCollapse,
    /// Open the full-view modal (no truncation).
    OpenFullView,
    /// Close the full-view modal.
    CloseFullView,
    /// Copy the selected request as cURL (redacted headers).
    CopyCurl,
    /// Copy the selected request as cURL including raw header values.
    CopyCurlRaw,

    // ── TASK-203 ─────────────────────────────────────────────────────
    /// Open the URL filter input field.
    BeginFilterInput,
    /// Commit the current filter draft.
    CommitFilterInput,
    /// Cancel the current filter draft.
    CancelFilterInput,
    /// Append `char` to the current filter draft (only while in input mode).
    FilterChar(char),
    /// Delete the last character from the filter draft.
    FilterBackspace,
    /// Toggle the named HTTP method chip.
    ToggleMethod(&'static str),
    /// Toggle the named status-class chip.
    ToggleStatus(StatusClass),
    /// Toggle the global pause state.
    TogglePause,
    /// Open the clear-confirmation modal.
    BeginClearConfirm,
    /// Confirm and clear the request buffer.
    ConfirmClear,
    /// Dismiss the clear-confirmation modal.
    CancelClearConfirm,
    /// Reset every filter (URL substring, method chips, status chips)
    /// in one shot. Bound to `X` on the keyboard and to the `[clear filters]`
    /// chip in the filter bar.
    ClearAllFilters,
    /// Copy the visible row at the given index as a cURL command (in
    /// the **redacted** mode — matches the `y` keybind). Fired by
    /// clicking the `[y]` glyph at the end of a row, so users don't
    /// have to select first. `Y` (shift-y) still copies the currently
    /// selected row raw.
    CopyCurlForRow(usize),

    /// Toggle crossterm mouse capture. When off, the terminal handles
    /// the mouse natively so users can select text to copy. Triggered
    /// by the `M` keybind.
    ToggleMouseCapture,

    /// Ignored input — the main loop receives this and skips the render.
    NoOp,
}

/// Convert a raw crossterm event into an `AppEvent`.
///
/// Unknown key / mouse sequences fall through to `AppEvent::NoOp` rather
/// than panicking, so the main loop stays silent under unusual input.
#[must_use]
pub fn translate(
    event: CrosstermEvent,
    hit_regions: &[HitRegion],
    input_mode: bool,
    confirm_clear_mode: bool,
    full_view_mode: bool,
) -> AppEvent {
    match event {
        CrosstermEvent::Key(key) => {
            map_key(key, input_mode, confirm_clear_mode, full_view_mode).unwrap_or(AppEvent::NoOp)
        }
        CrosstermEvent::Mouse(m) => dispatch_mouse(m, hit_regions).unwrap_or(AppEvent::NoOp),
        CrosstermEvent::Resize(_, _) => AppEvent::Resize,
        _ => AppEvent::NoOp,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_id_next_wraps() {
        assert_eq!(PaneId::List.next(), PaneId::Detail);
        assert_eq!(PaneId::Detail.next(), PaneId::List);
    }

    #[test]
    fn translate_resize_is_resize() {
        let ev = CrosstermEvent::Resize(80, 24);
        assert_eq!(translate(ev, &[], false, false, false), AppEvent::Resize);
    }
}
