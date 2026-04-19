//! Bottom status bar.
//!
//! Two short lines fused into a single ratatui `Paragraph`:
//!   1. Listen addr + counters (connections, buffered rows, paused flag).
//!   2. Keybinding hint (extended in TASK-202/203).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

use super::super::app::App;
use super::super::theme::Theme;

/// Render the status bar inside the provided rect.
pub fn render(area: Rect, buf: &mut Buffer, app: &App, theme: &Theme) {
    let pause_badge = if app.paused { " · PAUSED" } else { "" };
    let filter_badge = if app.filter.is_empty() {
        String::new()
    } else {
        format!(" · filter: {}", filter_summary(app))
    };

    let status_line = if app.mock_mode {
        format!(
            " {badge}  mock data — {rows} rows — not connected to any server{pause}{filter} ",
            badge = "rustotron",
            rows = app.rows.len(),
            pause = pause_badge,
            filter = filter_badge,
        )
    } else {
        format!(
            " {badge}  listening on {addr}  —  clients: {clients}  —  rows: {rows}{pause}{filter} ",
            badge = "rustotron",
            addr = app.listen_addr,
            clients = app.connections.count(),
            rows = app.rows.len(),
            pause = pause_badge,
            filter = filter_badge,
        )
    };

    let hints = " q/Ctrl+C: quit · j/k or ↑/↓: nav · tab: pane · / url · 1-5 methods · \
                  Alt+1-4 status · p pause · c clear · z collapse · F full · y cURL (Y raw) ";

    let lines = vec![
        Line::styled(status_line, theme.status_bar),
        Line::styled(hints.to_string(), theme.status_hint),
    ];
    Paragraph::new(lines).render(area, buf);
}

fn filter_summary(app: &App) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !app.filter.url_substring.is_empty() {
        parts.push(format!("url~\"{}\"", app.filter.url_substring));
    }
    if !app.filter.methods.is_empty() {
        let mut m: Vec<&str> = app.filter.methods.iter().map(String::as_str).collect();
        m.sort_unstable();
        parts.push(m.join(","));
    }
    if !app.filter.status_classes.is_empty() {
        let mut s: Vec<&'static str> = app
            .filter
            .status_classes
            .iter()
            .map(|c| c.label())
            .collect();
        s.sort_unstable();
        parts.push(s.join(","));
    }
    parts.join(" ")
}

/// Build the short display string used by the banner. Public so the
/// caller can reuse it in logs / demos.
#[must_use]
pub fn listen_banner(listen_addr: &str, clients: usize, rows: usize) -> String {
    format!("listening on {listen_addr} | clients: {clients} | rows: {rows}")
}
