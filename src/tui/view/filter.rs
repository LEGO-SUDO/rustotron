//! Filter bar — URL input, method chips, status-class chips, pause,
//! clear.
//!
//! Each chip / button registers a [`HitRegion`] so every action is
//! reachable via mouse alone. Keyboard equivalents live in
//! [`super::super::keys`].

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use super::super::app::App;
use super::super::filter::StatusClass;
use super::super::mouse::{Action, HitRegion};
use super::super::theme::Theme;

const METHODS: &[(&str, char)] = &[
    ("GET", '1'),
    ("POST", '2'),
    ("PUT", '3'),
    ("DELETE", '4'),
    ("PATCH", '5'),
];

const STATUS_CHIPS: &[(StatusClass, &str)] = &[
    (StatusClass::Success, "2xx"),
    (StatusClass::Redirect, "3xx"),
    (StatusClass::ClientError, "4xx"),
    (StatusClass::ServerError, "5xx"),
];

/// Render the filter bar in the given single-row `area`.
pub fn render(area: Rect, buf: &mut Buffer, app: &mut App, theme: &Theme) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    // Build the line as a list of (text, style, optional-action) tuples
    // so we can register hit regions at precise column ranges.
    let mut segments: Vec<(String, Style, Option<Action>)> = Vec::new();

    // URL input chip.
    let url_label = if app.filter_input_mode {
        format!("/{}", app.filter_draft)
    } else if app.filter.url_substring.is_empty() {
        "/url".to_string()
    } else {
        format!("/{}", app.filter.url_substring)
    };
    let url_style = if app.filter_input_mode {
        Style::new()
            .add_modifier(Modifier::BOLD)
            .add_modifier(Modifier::REVERSED)
    } else if !app.filter.url_substring.is_empty() {
        Style::new().add_modifier(Modifier::BOLD)
    } else {
        theme.dim
    };
    let mut url_text = format!(" [{url_label}] ");
    if app.filter_input_mode {
        // Render a simple cursor glyph at the end of the draft so tests
        // can assert input mode is active.
        url_text = format!(" [{url_label}_] ");
    }
    segments.push((url_text, url_style, Some(Action::BeginFilterInput)));

    // Method chips.
    for (method, _key) in METHODS {
        let selected = app.filter.methods.contains(*method);
        let style = chip_style(selected, theme);
        let text = format!("[{method}]");
        segments.push((
            format!(" {text} "),
            style,
            Some(Action::ToggleMethod(method)),
        ));
    }

    // Status-class chips.
    for (class, label) in STATUS_CHIPS {
        let selected = app.filter.status_classes.contains(class);
        let style = chip_style(selected, theme);
        segments.push((
            format!(" [{label}] "),
            style,
            Some(Action::ToggleStatus(*class)),
        ));
    }

    // Pause button.
    let pause_label = if app.paused { "[paused]" } else { "[p]" };
    let pause_style = chip_style(app.paused, theme);
    segments.push((
        format!(" {pause_label} "),
        pause_style,
        Some(Action::TogglePause),
    ));

    // Clear button.
    segments.push((
        " [c clear] ".to_string(),
        theme.dim,
        Some(Action::BeginClearConfirm),
    ));

    // Layout — iterate segments, capturing their cell ranges for hit
    // regions, then render as a single `Paragraph`.
    let mut x = area.x;
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (text, style, action) in segments {
        let width = text.chars().count();
        let width_u16 = u16::try_from(width).unwrap_or(u16::MAX);
        if let Some(a) = action {
            // Clamp hit region to the filter bar row.
            let region_width = width_u16.min(area.x.saturating_add(area.width).saturating_sub(x));
            if region_width > 0 {
                let rect = Rect::new(x, area.y, region_width, 1);
                app.hit_regions.push(HitRegion::new(rect, a));
            }
        }
        spans.push(Span::styled(text.clone(), style));
        x = x.saturating_add(width_u16);
        if x >= area.x.saturating_add(area.width) {
            break;
        }
    }

    let line = Line::from(spans);
    Paragraph::new(vec![line]).render(area, buf);
}

fn chip_style(selected: bool, theme: &Theme) -> Style {
    if selected {
        Style::new()
            .add_modifier(Modifier::BOLD)
            .add_modifier(Modifier::REVERSED)
    } else {
        theme.dim
    }
}
