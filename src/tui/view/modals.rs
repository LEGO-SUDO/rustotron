//! Modal overlays: detail full-view + clear-confirmation.
//!
//! Modals are drawn on top of the root view. They register their own
//! [`HitRegion`]s so background clicks are absorbed rather than falling
//! through to the list / detail panes.

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap};

use super::super::app::App;
use super::super::highlight;
use super::super::mouse::{Action, HitRegion};
use super::super::theme::Theme;
use super::detail;

/// Draw the detail-pane full-view modal inside `area` (the detail pane's
/// rect). Replaces the detail renderer when `App::detail_full_view` is
/// true.
pub fn render_full_view(area: Rect, buf: &mut Buffer, app: &mut App, theme: &Theme) {
    Clear.render(area, buf);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Detail — full view (Esc to close) ")
        .border_style(theme.focused_border);

    let Some(req) = app.selected_request().cloned() else {
        Paragraph::new("(no request selected)")
            .block(block)
            .alignment(Alignment::Center)
            .style(theme.empty_state)
            .render(area, buf);
        return;
    };

    // Absorb background clicks inside the modal area.
    app.hit_regions.push(HitRegion::new(
        area,
        Action::FocusPane(super::super::event::PaneId::Detail),
    ));

    let plain = theme.focused_border == Style::new();
    let body = detail::pretty_body_string(&req);
    let lines = highlight::highlight_json_lines(&body, plain);

    Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((app.detail_scroll, 0))
        .render(area, buf);
}

/// Draw the "Clear all requests? [y/n]" confirmation modal centred on
/// the given root `area`.
pub fn render_confirm_clear(area: Rect, buf: &mut Buffer, app: &mut App, theme: &Theme) {
    let modal_width: u16 = 46;
    let modal_height: u16 = 5;
    let Some(rect) = centre_rect(area, modal_width, modal_height) else {
        return;
    };

    Clear.render(rect, buf);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Clear requests ")
        .border_style(theme.focused_border);

    // Two lines of content: prompt + button row.
    let prompt = Line::from(vec![Span::styled(
        "Clear all requests? This cannot be undone.",
        Style::new().add_modifier(Modifier::BOLD),
    )]);

    let yes = "[y — yes]";
    let no = "[n — cancel]";
    let buttons_spans: Vec<Span<'static>> = vec![
        Span::raw("  "),
        Span::styled(yes.to_string(), Style::new().add_modifier(Modifier::BOLD)),
        Span::raw("   "),
        Span::styled(no.to_string(), theme.dim),
    ];
    let buttons = Line::from(buttons_spans);

    Paragraph::new(vec![Line::raw(""), prompt, buttons])
        .block(block)
        .alignment(Alignment::Center)
        .render(rect, buf);

    // Register the absorber FIRST, then the yes/no buttons on top. Mouse
    // dispatch iterates in reverse so the later (more specific) regions
    // win — without this ordering, every click in the modal (including
    // on `[y — yes]`) falls through to the background absorber and cancels.
    app.hit_regions
        .push(HitRegion::new(rect, Action::CancelClearConfirm));

    // Button hit-regions. The button row is inside the block, with a
    // 1-cell top border + 1 blank line above it, so row y+3. We rely on
    // column alignment being approximate; clicks anywhere in the bottom
    // half of the modal map to cancel or confirm based on which half.
    let inner_y = rect.y.saturating_add(3);
    let half_w = rect.width / 2;
    if half_w > 2 {
        let yes_rect = Rect::new(
            rect.x.saturating_add(1),
            inner_y,
            half_w.saturating_sub(1),
            1,
        );
        let no_rect = Rect::new(
            rect.x.saturating_add(half_w),
            inner_y,
            rect.width.saturating_sub(half_w).saturating_sub(1),
            1,
        );
        app.hit_regions
            .push(HitRegion::new(yes_rect, Action::ConfirmClear));
        app.hit_regions
            .push(HitRegion::new(no_rect, Action::CancelClearConfirm));
    }
}

fn centre_rect(area: Rect, width: u16, height: u16) -> Option<Rect> {
    if area.width < width || area.height < height {
        return None;
    }
    let x = area.x.saturating_add(area.width.saturating_sub(width) / 2);
    let y = area
        .y
        .saturating_add(area.height.saturating_sub(height) / 2);
    Some(Rect::new(x, y, width, height))
}
