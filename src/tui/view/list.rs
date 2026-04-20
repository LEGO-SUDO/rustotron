//! Request list pane (left).
//!
//! Renders each row as `HH:MM:SS METHOD STATUS DURATION URL`. Status is
//! colour-coded via the active [`Theme`]. A [`HitRegion`] is registered
//! per rendered row so mouse clicks resolve to
//! [`super::super::event::AppEvent::SelectRow`].
//!
//! The pane is stateful — it uses ratatui's `ListState` so the selection
//! highlight and viewport offset survive re-renders.

use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, StatefulWidget, Widget};

use crate::store::Request;

use super::super::app::App;
use super::super::event::PaneId;
use super::super::mouse::{Action, HitRegion};
use super::super::theme::Theme;

/// Draw the request list pane. Mutates `app` only to push hit regions.
pub fn render(area: Rect, buf: &mut Buffer, app: &mut App, theme: &Theme) {
    let focused = app.focused == PaneId::List;
    let visible_count = app.visible_rows().len();
    let total_count = app.rows.len();
    let title = if visible_count == total_count {
        format!(" Requests ({visible_count}) ")
    } else {
        format!(" Requests ({visible_count}/{total_count}) ")
    };
    let border_style = if focused {
        theme.focused_border
    } else {
        theme.unfocused_border
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(border_style);

    if app.visible_rows().is_empty() {
        render_empty(area, buf, block, app, theme);
        return;
    }

    // Build items into owned lines so we can mutate `app.hit_regions`
    // without holding a borrow back into `app.visible_rows()`.
    let items: Vec<ListItem<'static>> = app
        .visible_rows()
        .iter()
        .map(|req| row_to_item(req, theme))
        .collect();
    let visible_len = app.visible_rows().len();

    // Register a broad pane-click hit region first, then per-row overlays.
    app.hit_regions
        .push(HitRegion::new(area, Action::FocusPane(PaneId::List)));
    let inner = block.inner(area);
    // The first `ListState.offset()` rows are above the viewport; we have
    // no public access to it, so we let ratatui render and use the
    // selected index to anchor the hit rectangles. This is a small
    // fidelity compromise (clicks on rows above the selected scroll
    // window still fire), acceptable at v1 given the compact pane size.
    for idx in 0..visible_len {
        let y_offset = u16::try_from(idx).unwrap_or(u16::MAX);
        let Some(y) = inner.y.checked_add(y_offset) else {
            break;
        };
        if y >= inner.y.saturating_add(inner.height) {
            break;
        }
        let row_rect = Rect::new(inner.x, y, inner.width, 1);
        app.hit_regions
            .push(HitRegion::new(row_rect, Action::SelectRow(idx)));
    }

    let list = List::new(items)
        .block(block)
        .highlight_style(theme.row_highlight)
        .highlight_symbol("▸ ");
    StatefulWidget::render(list, area, buf, &mut app.list_state);

    // Overlay a per-row `[y]` copy glyph at the right edge of each
    // rendered row. Registered AFTER the SelectRow hit regions so
    // reverse-iteration dispatch picks the narrower copy region when
    // clicks land on the glyph.
    overlay_row_copy_glyphs(inner, buf, app, theme, visible_len);
}

/// Paint a `[y]` marker at the right edge of every visible row and
/// register a `CopyRowAsCurl` hit region over it. Bail gracefully if the
/// pane is narrower than the glyph (`[y]` + one cell of breathing room).
fn overlay_row_copy_glyphs(
    inner: Rect,
    buf: &mut Buffer,
    app: &mut App,
    theme: &Theme,
    visible_len: usize,
) {
    const GLYPH: &str = "[y]";
    const GLYPH_WIDTH: u16 = 3;
    if inner.width < GLYPH_WIDTH + 1 {
        return;
    }
    let right_edge = inner.x.saturating_add(inner.width);
    let glyph_x = right_edge.saturating_sub(GLYPH_WIDTH);
    for idx in 0..visible_len {
        let y_offset = u16::try_from(idx).unwrap_or(u16::MAX);
        let Some(y) = inner.y.checked_add(y_offset) else {
            break;
        };
        if y >= inner.y.saturating_add(inner.height) {
            break;
        }
        buf.set_string(glyph_x, y, GLYPH, theme.dim);
        let rect = Rect::new(glyph_x, y, GLYPH_WIDTH, 1);
        app.hit_regions
            .push(HitRegion::new(rect, Action::CopyRowAsCurl(idx)));
    }
}

fn render_empty(area: Rect, buf: &mut Buffer, block: Block<'_>, app: &mut App, theme: &Theme) {
    app.hit_regions
        .push(HitRegion::new(area, Action::FocusPane(PaneId::List)));
    let text: &str = if !app.rows.is_empty() {
        "no rows match the current filter"
    } else if app.mock_mode {
        "no mock rows — something went wrong initialising the demo fixture"
    } else {
        "waiting for an RN app to connect…"
    };
    let para = Paragraph::new(text)
        .block(block)
        .alignment(Alignment::Center)
        .style(theme.empty_state);
    para.render(area, buf);
}

fn row_to_item(req: &Request, theme: &Theme) -> ListItem<'static> {
    let hhmmss = format_hhmmss(req.received_at);
    let method = req.exchange.request.method.as_deref().unwrap_or("???");
    let status = req.exchange.response.status;
    let duration = req
        .exchange
        .duration
        .map_or_else(|| "     ?".to_string(), |d| format!("{d:>5.0}ms"));
    let url = short_url(&req.exchange.request.url);

    let status_str = format!("{status:<3}");
    let status_style = theme.status_style(status);
    let dim = theme.dim;
    let reset = Style::new();

    let line = Line::from(vec![
        Span::styled(hhmmss, dim),
        Span::raw(" "),
        Span::styled(format!("{method:<6}"), reset),
        Span::raw(" "),
        Span::styled(status_str, status_style),
        Span::raw(" "),
        Span::styled(duration, dim),
        Span::raw(" "),
        Span::styled(url, reset),
    ]);
    ListItem::new(line)
}

fn short_url(url: &str) -> String {
    // Strip scheme + host so the most meaningful path/query bit gets the
    // room. Falls back to the full string for anything non-URL-looking.
    if let Some(scheme_end) = url.find("://") {
        let after = &url[scheme_end + 3..];
        if let Some(slash) = after.find('/') {
            return after[slash..].to_string();
        }
    }
    url.to_string()
}

fn format_hhmmss(t: SystemTime) -> String {
    match t.duration_since(UNIX_EPOCH) {
        Ok(dur) => {
            let total = dur.as_secs();
            let h = (total / 3600) % 24;
            let m = (total / 60) % 60;
            let s = total % 60;
            format!("{h:02}:{m:02}:{s:02}")
        }
        Err(_) => "??:??:??".to_string(),
    }
}
