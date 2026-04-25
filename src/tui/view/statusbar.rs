//! Bottom status bar.
//!
//! Two short lines fused into a single ratatui `Paragraph`:
//!   1. Listen addr + counters (clients, rows, memory, pause, filter summary).
//!   2. Either the toast message (when one is active) or the keybinding
//!      hint. Toast auto-expires after ~2 s via `App::tick_toast`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

use super::super::app::{App, ToastKind};
use super::super::theme::Theme;

/// Static hint line shown when no toast is active.
const HINT_LINE: &str = " q/Ctrl+C: quit · tab: pane · j/k or ↑/↓: nav (list) / scroll body (detail) · \
      PgUp/PgDn: body · / url · 1-5 methods · Alt+1-4 status · X clear filters · \
      p pause · c clear · z collapse · F full · y cURL (Y raw) · b body · click [y] row · M select-text ";

/// Render the status bar inside the provided rect.
pub fn render(area: Rect, buf: &mut Buffer, app: &App, theme: &Theme) {
    let pause_badge = if app.paused { " · PAUSED" } else { "" };
    let filter_badge = if app.filter.is_empty() {
        String::new()
    } else {
        format!(" · filter: {}", filter_summary(app))
    };
    let mem_badge = format_memory_badge();

    let status_line = if app.mock_mode {
        format!(
            " {badge}  mock data — {rows} rows — not connected to any server{pause}{filter}{mem} ",
            badge = "rustotron",
            rows = app.rows.len(),
            pause = pause_badge,
            filter = filter_badge,
            mem = mem_badge,
        )
    } else {
        format!(
            " {badge}  listening on {addr}  —  clients: {clients}  —  rows: {rows}{pause}{filter}{mem} ",
            badge = "rustotron",
            addr = app.listen_addr,
            clients = app.connections.count(),
            rows = app.rows.len(),
            pause = pause_badge,
            filter = filter_badge,
            mem = mem_badge,
        )
    };

    // Second line: toast (when live) overrides the hint line. The toast
    // is styled per-kind (success / error / info); the hint uses the
    // theme's muted style.
    let second_line = match app.current_toast.as_ref() {
        Some(toast) => Line::styled(
            format!(
                " {glyph} {msg} ",
                glyph = toast.kind.glyph(),
                msg = toast.message
            ),
            toast_style(toast.kind, theme),
        ),
        None => Line::styled(HINT_LINE.to_string(), theme.status_hint),
    };

    let lines = vec![Line::styled(status_line, theme.status_bar), second_line];
    Paragraph::new(lines).render(area, buf);
}

/// Build the ` · mem: N.N MB` chip. Reads RSS via `memory-stats` and
/// formats in decimal MB (1 MB = 1_000_000 bytes — easier to eyeball
/// than MiB). Returns an empty string if the platform doesn't expose
/// RSS so the rest of the status line doesn't shift.
///
/// Tests that need deterministic output set
/// `RUSTOTRON_FAKE_MEM_MB=<number>` to pin the displayed value. Never
/// honoured in release binaries because the env var is only read here.
fn format_memory_badge() -> String {
    if let Ok(fake) = std::env::var("RUSTOTRON_FAKE_MEM_MB") {
        return format!("  —  mem: {fake} MB");
    }
    match memory_stats::memory_stats() {
        Some(stats) => {
            // `physical_mem` is the RSS equivalent (bytes).
            let mb = stats.physical_mem as f64 / 1_000_000.0;
            format!("  —  mem: {mb:.1} MB")
        }
        None => String::new(),
    }
}

fn toast_style(kind: ToastKind, theme: &Theme) -> Style {
    // Prefer the paired status-code palette so the toast colour matches
    // the rest of the TUI. Fall back to the theme defaults when the
    // active palette is the monochrome `plain` one (e.g. `NO_COLOR`).
    let fg = match kind {
        ToastKind::Success => theme.status_success.fg.unwrap_or(Color::Green),
        ToastKind::Error => theme.status_server_err.fg.unwrap_or(Color::Red),
        ToastKind::Info => theme.status_redirect.fg.unwrap_or(Color::Cyan),
    };
    Style::new().fg(fg).add_modifier(Modifier::BOLD)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_badge_formats_as_non_empty_on_supported_platforms() {
        let badge = format_memory_badge();
        // On macOS + Linux (our launch targets), RSS is always available
        // so the badge should start with "  —  mem:". If a future target
        // lacks memory_stats support, the function returns "" and this
        // assertion would flip — we'd notice.
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            assert!(badge.contains("mem:"), "got: {badge:?}");
            assert!(badge.ends_with(" MB"));
        }
    }

    #[test]
    fn toast_style_bold_and_coloured() {
        let theme = Theme::dark();
        let s = toast_style(ToastKind::Success, &theme);
        assert!(s.add_modifier.contains(Modifier::BOLD));
        assert!(s.fg.is_some());
    }
}
