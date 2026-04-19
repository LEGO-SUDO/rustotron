//! Mouse hit-testing.
//!
//! Ratatui is immediate-mode — widgets compute their own `Rect`s during
//! render and discard them. To map a click back to a semantic action we
//! have to remember those rects ourselves. Each frame, the view layer
//! pushes a [`HitRegion`] for every clickable thing onto the hit-region
//! list that [`super::app::App`] owns; when a mouse event arrives, we
//! iterate the list and dispatch the first match to
//! [`super::event::AppEvent`].
//!
//! Rects are checked in order — later-pushed regions take precedence over
//! earlier ones. That lets overlay modals reliably absorb clicks without
//! the underlying panes seeing them.

use crossterm::event::{MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use super::event::{AppEvent, PaneId};
use super::filter::StatusClass;

/// Domain-level action a mouse click can trigger. Mouse wheel scrolls are
/// handled separately — they do not go through the hit table because they
/// want to affect whichever pane contains the cursor.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Action {
    /// Select a specific row of the list view (index into the displayed
    /// rows, not into the underlying store).
    SelectRow(usize),
    /// Focus a pane. Used when the click lands on an empty area inside a
    /// pane that doesn't have a more specific target.
    FocusPane(PaneId),
    /// Trigger the "copy cURL" button in the detail pane.
    CopyCurl,
    /// Begin filter URL input mode (clicking on `[url]` chip).
    BeginFilterInput,
    /// Toggle the named method chip.
    ToggleMethod(&'static str),
    /// Toggle the named status-class chip.
    ToggleStatus(StatusClass),
    /// Toggle the pause flag.
    TogglePause,
    /// Open the clear-confirmation modal.
    BeginClearConfirm,
    /// Confirm and clear.
    ConfirmClear,
    /// Cancel the clear-confirmation modal.
    CancelClearConfirm,
}

/// A clickable rectangle registered by the view layer during render.
///
/// Later-pushed regions win (see module docs). A typical render pushes
/// broad pane regions first, then per-row overlays on top of them.
#[derive(Debug, Clone, Copy)]
pub struct HitRegion {
    /// The area this region covers, in terminal cells.
    pub rect: Rect,
    /// What to do when the click lands inside the rect.
    pub action: Action,
}

impl HitRegion {
    /// Convenience constructor.
    #[must_use]
    pub const fn new(rect: Rect, action: Action) -> Self {
        Self { rect, action }
    }
}

/// Resolve a mouse event against the registered hit regions.
///
/// Scroll-wheel events ignore the rect table entirely — they translate
/// straight into pane-agnostic scroll actions. Left-clicks and drags
/// iterate the table in **reverse** so later registrations (overlays)
/// win over earlier ones (panes).
#[must_use]
pub fn dispatch_mouse(event: MouseEvent, regions: &[HitRegion]) -> Option<AppEvent> {
    match event.kind {
        MouseEventKind::ScrollDown => Some(AppEvent::ScrollDown),
        MouseEventKind::ScrollUp => Some(AppEvent::ScrollUp),
        MouseEventKind::Down(_) => resolve_click(event.column, event.row, regions),
        _ => None,
    }
}

fn resolve_click(col: u16, row: u16, regions: &[HitRegion]) -> Option<AppEvent> {
    for region in regions.iter().rev() {
        if contains(region.rect, col, row) {
            return Some(match region.action {
                Action::SelectRow(idx) => AppEvent::SelectRow(idx),
                Action::FocusPane(pane) => AppEvent::FocusPane(pane),
                Action::CopyCurl => AppEvent::CopyCurl,
                Action::BeginFilterInput => AppEvent::BeginFilterInput,
                Action::ToggleMethod(m) => AppEvent::ToggleMethod(m),
                Action::ToggleStatus(c) => AppEvent::ToggleStatus(c),
                Action::TogglePause => AppEvent::TogglePause,
                Action::BeginClearConfirm => AppEvent::BeginClearConfirm,
                Action::ConfirmClear => AppEvent::ConfirmClear,
                Action::CancelClearConfirm => AppEvent::CancelClearConfirm,
            });
        }
    }
    None
}

#[inline]
fn contains(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.x
        && y >= rect.y
        && x < rect.x.saturating_add(rect.width)
        && y < rect.y.saturating_add(rect.height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{MouseButton, MouseEventKind};

    fn click(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        }
    }

    #[test]
    fn scroll_wheel_bypasses_hit_table() {
        let ev = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };
        assert_eq!(dispatch_mouse(ev, &[]), Some(AppEvent::ScrollDown));
    }

    #[test]
    fn click_outside_any_region_is_none() {
        let region = HitRegion::new(Rect::new(0, 0, 10, 5), Action::FocusPane(PaneId::List));
        assert!(dispatch_mouse(click(50, 50), &[region]).is_none());
    }

    #[test]
    fn click_inside_single_region_dispatches_action() {
        let region = HitRegion::new(Rect::new(2, 2, 10, 5), Action::FocusPane(PaneId::Detail));
        assert_eq!(
            dispatch_mouse(click(3, 3), &[region]),
            Some(AppEvent::FocusPane(PaneId::Detail))
        );
    }

    #[test]
    fn click_on_boundary_is_inside() {
        // rect x=0 y=0 w=1 h=1 covers exactly cell (0,0).
        let region = HitRegion::new(Rect::new(0, 0, 1, 1), Action::SelectRow(0));
        assert_eq!(
            dispatch_mouse(click(0, 0), &[region]),
            Some(AppEvent::SelectRow(0))
        );
    }

    #[test]
    fn click_past_boundary_is_outside() {
        let region = HitRegion::new(Rect::new(0, 0, 2, 2), Action::SelectRow(0));
        assert!(dispatch_mouse(click(2, 0), &[region]).is_none());
        assert!(dispatch_mouse(click(0, 2), &[region]).is_none());
    }

    #[test]
    fn later_registered_region_wins_over_earlier_overlap() {
        let pane = HitRegion::new(Rect::new(0, 0, 10, 10), Action::FocusPane(PaneId::List));
        let row = HitRegion::new(Rect::new(0, 3, 10, 1), Action::SelectRow(7));
        let regions = [pane, row];
        assert_eq!(
            dispatch_mouse(click(5, 3), &regions),
            Some(AppEvent::SelectRow(7))
        );
        // Clicking outside the row overlay but inside the pane still focuses the pane.
        assert_eq!(
            dispatch_mouse(click(5, 0), &regions),
            Some(AppEvent::FocusPane(PaneId::List))
        );
    }

    #[test]
    fn select_row_action_translates_to_select_row_event() {
        let region = HitRegion::new(Rect::new(0, 0, 5, 1), Action::SelectRow(42));
        assert_eq!(
            dispatch_mouse(click(1, 0), &[region]),
            Some(AppEvent::SelectRow(42))
        );
    }

    #[test]
    fn copy_curl_click_translates() {
        let region = HitRegion::new(Rect::new(0, 0, 10, 1), Action::CopyCurl);
        assert_eq!(
            dispatch_mouse(click(1, 0), &[region]),
            Some(AppEvent::CopyCurl)
        );
    }

    #[test]
    fn filter_chip_clicks_translate() {
        let region = HitRegion::new(Rect::new(0, 0, 6, 1), Action::ToggleMethod("POST"));
        assert_eq!(
            dispatch_mouse(click(3, 0), &[region]),
            Some(AppEvent::ToggleMethod("POST"))
        );
    }
}
