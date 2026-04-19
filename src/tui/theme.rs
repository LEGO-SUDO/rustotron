//! Colour palette used by every view.
//!
//! The TUI uses a subtle dark palette with paired `focused` / `unfocused`
//! tokens and status-class colouring. Respects `NO_COLOR` — when set, a
//! monochrome [`Theme`] is returned where every field is `Style::reset()`
//! so the renderer emits no ANSI escapes.
//!
//! Theme is a plain value, not a global. Callers construct one per render
//! via [`Theme::from_env`] (or pass [`Theme::plain`] explicitly from
//! tests). This keeps the render pipeline pure and snapshot-friendly.

use ratatui::style::{Color, Modifier, Style};

/// Paired colour tokens used across the view tree.
///
/// Fields are `Style`s rather than `Color`s so individual tokens can carry
/// `Modifier::BOLD` etc. independent of their colour.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    /// Border / title of the pane that currently has focus.
    pub focused_border: Style,
    /// Border / title of any other pane.
    pub unfocused_border: Style,
    /// List row that is currently selected.
    pub row_highlight: Style,
    /// Status bar background line (entire line).
    pub status_bar: Style,
    /// Hint text under the status bar (keybinds).
    pub status_hint: Style,
    /// Colour for 2xx status codes.
    pub status_success: Style,
    /// Colour for 3xx status codes.
    pub status_redirect: Style,
    /// Colour for 4xx status codes.
    pub status_client_err: Style,
    /// Colour for 5xx status codes.
    pub status_server_err: Style,
    /// Dim text colour used for timestamps / helper columns.
    pub dim: Style,
    /// Text used to draw the empty-state prompt.
    pub empty_state: Style,
}

impl Theme {
    /// The coloured dark-background palette. Used when colour is enabled.
    #[must_use]
    pub const fn dark() -> Self {
        Self {
            focused_border: Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            unfocused_border: Style::new().fg(Color::DarkGray),
            row_highlight: Style::new()
                .bg(Color::Rgb(38, 50, 68))
                .add_modifier(Modifier::BOLD),
            status_bar: Style::new().fg(Color::White).bg(Color::Rgb(24, 28, 36)),
            status_hint: Style::new().fg(Color::DarkGray),
            status_success: Style::new().fg(Color::Green),
            status_redirect: Style::new().fg(Color::Cyan),
            status_client_err: Style::new().fg(Color::Yellow),
            status_server_err: Style::new().fg(Color::Red),
            dim: Style::new().fg(Color::DarkGray),
            empty_state: Style::new()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        }
    }

    /// Monochrome palette — every token is [`Style::reset`]. Used when
    /// `NO_COLOR` is set in the environment, or by tests that want
    /// deterministic snapshots.
    #[must_use]
    pub const fn plain() -> Self {
        let s = Style::new();
        Self {
            focused_border: s,
            unfocused_border: s,
            row_highlight: Style::new().add_modifier(Modifier::REVERSED),
            status_bar: s,
            status_hint: s,
            status_success: s,
            status_redirect: s,
            status_client_err: s,
            status_server_err: s,
            dim: s,
            empty_state: Style::new().add_modifier(Modifier::ITALIC),
        }
    }

    /// Choose a palette based on the process environment.
    ///
    /// Reads `NO_COLOR`; when present (regardless of value), returns
    /// [`Theme::plain`]. Otherwise returns [`Theme::dark`]. Callers that
    /// want to force one explicitly (e.g. tests) should use the
    /// constructors directly.
    #[must_use]
    pub fn from_env() -> Self {
        if std::env::var_os("NO_COLOR").is_some() {
            Self::plain()
        } else {
            Self::dark()
        }
    }

    /// Resolve the style for a given HTTP status code class.
    #[must_use]
    pub fn status_style(&self, status: u16) -> Style {
        match status / 100 {
            2 => self.status_success,
            3 => self.status_redirect,
            4 => self.status_client_err,
            5 => self.status_server_err,
            _ => self.dim,
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::from_env()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_theme_has_reset_borders() {
        let t = Theme::plain();
        assert_eq!(t.focused_border, Style::new());
        assert_eq!(t.unfocused_border, Style::new());
    }

    #[test]
    fn status_style_picks_matching_class() {
        let t = Theme::dark();
        assert_eq!(t.status_style(200), t.status_success);
        assert_eq!(t.status_style(301), t.status_redirect);
        assert_eq!(t.status_style(404), t.status_client_err);
        assert_eq!(t.status_style(503), t.status_server_err);
        assert_eq!(t.status_style(100), t.dim);
    }
}
