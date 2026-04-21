//! Root view layout + per-pane renderers.
//!
//! The root layout is:
//!
//!   - top 1 row: filter bar (URL input, method chips, status chips,
//!     pause, clear — TASK-203)
//!   - middle: 40% list / 60% detail
//!   - bottom 2 rows: status bar (line 1 = state, line 2 = hints)
//!
//! When the full-view modal is open (TASK-202), the detail area is
//! replaced by the modal renderer; all other regions still render.
//!
//! Views push [`super::mouse::HitRegion`]s onto
//! [`super::app::App::hit_regions`] during render; the main loop
//! consults this table on the next mouse event.
//!
//! Call [`draw`] once per frame from the main loop.

pub mod detail;
pub mod filter;
pub mod list;
pub mod modals;
pub mod statusbar;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};

use super::app::App;
use super::theme::Theme;

/// Draw one frame. Clears the hit-region table first so click targets
/// always reflect the most recently rendered layout.
pub fn draw(frame: &mut Frame<'_>, app: &mut App, theme: &Theme) {
    app.hit_regions.clear();

    let area = frame.area();

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // filter bar
            Constraint::Min(3),    // panes
            Constraint::Length(2), // status bar
        ])
        .split(area);
    let filter_area = vertical[0];
    let main_area = vertical[1];
    let status_area = vertical[2];

    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(main_area);
    let list_area = horizontal[0];
    let detail_area = horizontal[1];

    filter::render(filter_area, frame.buffer_mut(), app, theme);
    list::render(list_area, frame.buffer_mut(), app, theme);
    if app.detail_full_view {
        modals::render_full_view(detail_area, frame.buffer_mut(), app, theme);
    } else {
        detail::render(detail_area, frame.buffer_mut(), app, theme);
    }
    statusbar::render(status_area, frame.buffer_mut(), app, theme);

    if app.confirm_clear_mode {
        modals::render_confirm_clear(area, frame.buffer_mut(), app, theme);
    }
}
