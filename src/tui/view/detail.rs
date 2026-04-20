//! Request detail pane (right).
//!
//! Shows the selected request's method/URL, headers (request + response),
//! and bodies rendered as syntax-highlighted JSON. Sections are
//! collapsible (`z`). Bodies over 1 MB are truncated with a hint line and
//! a `[copy cURL]` button; the full body is available via the full-view
//! modal (`F`).

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget,
    Widget, Wrap,
};

use crate::store::Request;

use super::super::app::{App, SectionId};
use super::super::body::{DEFAULT_BODY_LIMIT, pretty_json_string, truncate_if_large};
use super::super::event::PaneId;
use super::super::highlight::highlight_json_lines;
use super::super::mouse::{Action, HitRegion};
use super::super::theme::Theme;

const COPY_CURL_LABEL: &str = "[copy cURL]";
const TRUNCATION_HINT: &str = "[body truncated — press F to open full view]";

/// Draw the detail pane.
pub fn render(area: Rect, buf: &mut Buffer, app: &mut App, theme: &Theme) {
    let focused = app.focused == PaneId::Detail;
    let border_style = if focused {
        theme.focused_border
    } else {
        theme.unfocused_border
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Detail ")
        .border_style(border_style);

    app.hit_regions
        .push(HitRegion::new(area, Action::FocusPane(PaneId::Detail)));

    let Some(req) = app.selected_request().cloned() else {
        let empty = Paragraph::new("(no request selected)")
            .block(block)
            .alignment(Alignment::Center)
            .style(theme.empty_state);
        empty.render(area, buf);
        return;
    };

    // Register a `[copy cURL]` button region inside the top-right area
    // of the pane, above the scrollbar column.
    let inner = block.inner(area);
    if inner.width >= COPY_CURL_LABEL.len() as u16 + 2 && inner.height >= 1 {
        let label_w = COPY_CURL_LABEL.len() as u16;
        let btn_x = inner.x.saturating_add(inner.width.saturating_sub(label_w));
        let btn_rect = Rect::new(btn_x, inner.y, label_w, 1);
        app.hit_regions
            .push(HitRegion::new(btn_rect, Action::CopyCurl));
    }

    let plain = matches!(border_style, s if s == Style::new()) || theme_is_plain(theme);
    let lines = build_detail_lines(&req, app, theme, plain);

    let total_lines = lines.len();

    // Reserve one column on the right edge for the scrollbar.
    let para_area = if inner.width > 1 {
        Rect::new(
            inner.x,
            inner.y,
            inner.width.saturating_sub(1),
            inner.height,
        )
    } else {
        inner
    };

    let para = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((app.detail_scroll, 0));
    // Render the block + paragraph manually so we keep the right-column
    // free for the scrollbar. The paragraph does NOT carry the block.
    block.render(area, buf);
    para.render(para_area, buf);

    // Draw scrollbar on the right edge of the pane, inside the border.
    if inner.width >= 1 && total_lines > 0 {
        let sb_area = Rect::new(
            inner.x.saturating_add(inner.width.saturating_sub(1)),
            inner.y,
            1,
            inner.height,
        );
        let mut state = ScrollbarState::new(total_lines).position(app.detail_scroll as usize);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None);
        StatefulWidget::render(scrollbar, sb_area, buf, &mut state);
    }
}

fn theme_is_plain(theme: &Theme) -> bool {
    // Plain theme has reset styles on borders and status entries.
    theme.focused_border == Style::new() && theme.dim == Style::new()
}

/// Public helper used by the full-view modal: produces the raw pretty
/// body string (stringified-JSON aware), prefering the response body
/// over the request body when both exist.
#[must_use]
pub fn pretty_body_string(req: &Request) -> String {
    let resp = pretty_json_string(&req.exchange.response.body);
    let req_body = pretty_json_string(&req.exchange.request.data);
    if resp == "(empty)" && req_body != "(empty)" {
        req_body
    } else {
        resp
    }
}

fn build_detail_lines<'a>(req: &Request, app: &App, theme: &Theme, plain: bool) -> Vec<Line<'a>> {
    let mut lines: Vec<Line<'a>> = Vec::new();
    let bold = Style::new().add_modifier(Modifier::BOLD);

    let method = req.exchange.request.method.as_deref().unwrap_or("???");
    let status = req.exchange.response.status;
    let duration = req
        .exchange
        .duration
        .map_or_else(|| "?".to_string(), |d| format!("{d:.0}ms"));

    // Header: method, url, status
    lines.push(Line::from(vec![
        Span::styled("Method:   ", bold),
        Span::raw(method.to_string()),
    ]));
    lines.push(Line::from(vec![
        Span::styled("URL:      ", bold),
        Span::raw(req.exchange.request.url.clone()),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Status:   ", bold),
        Span::styled(format!("{status}"), theme.status_style(status)),
        Span::raw("   "),
        Span::styled("Duration: ", bold),
        Span::styled(duration, theme.dim),
    ]));
    lines.push(Line::raw(""));

    // Sections.
    push_header_section(
        &mut lines,
        "Request Headers",
        SectionId::RequestHeaders,
        req.exchange.request.headers.as_ref(),
        app,
        theme,
    );
    push_body_section(
        &mut lines,
        "Request Body",
        SectionId::RequestBody,
        &req.exchange.request.data,
        app,
        plain,
    );
    push_header_section(
        &mut lines,
        "Response Headers",
        SectionId::ResponseHeaders,
        req.exchange.response.headers.as_ref(),
        app,
        theme,
    );
    push_body_section(
        &mut lines,
        "Response Body",
        SectionId::ResponseBody,
        &req.exchange.response.body,
        app,
        plain,
    );

    lines
}

fn section_glyph(collapsed: bool) -> &'static str {
    if collapsed { "▸ " } else { "▾ " }
}

fn push_header_section<'a>(
    lines: &mut Vec<Line<'a>>,
    title: &str,
    id: SectionId,
    headers: Option<&std::collections::HashMap<String, String>>,
    app: &App,
    theme: &Theme,
) {
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let collapsed = app.is_collapsed(id);
    let header = format!("{}{}", section_glyph(collapsed), title);
    lines.push(Line::styled(header, bold));
    if collapsed {
        lines.push(Line::raw(""));
        return;
    }
    match headers {
        None => lines.push(Line::styled("  (none)", theme.dim)),
        Some(h) if h.is_empty() => lines.push(Line::styled("  (none)", theme.dim)),
        Some(h) => {
            let mut keys: Vec<&String> = h.keys().collect();
            keys.sort();
            for k in keys {
                let v = h.get(k).cloned().unwrap_or_default();
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(format!("{k}: "), bold),
                    Span::raw(v),
                ]));
            }
        }
    }
    lines.push(Line::raw(""));
}

fn push_body_section<'a>(
    lines: &mut Vec<Line<'a>>,
    title: &str,
    id: SectionId,
    value: &crate::protocol::Body,
    app: &App,
    plain: bool,
) {
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let collapsed = app.is_collapsed(id);
    let header = format!("{}{}", section_glyph(collapsed), title);
    lines.push(Line::styled(header, bold));
    if collapsed {
        lines.push(Line::raw(""));
        return;
    }

    let pretty = pretty_json_string(value);
    let (rendered, truncated) = truncate_if_large(pretty, DEFAULT_BODY_LIMIT);
    let highlighted = highlight_json_lines(&rendered, plain);
    for line in highlighted {
        // Preserve any styling on each highlighted span and indent by two
        // spaces so the body visually lives under the section heading.
        let mut spans: Vec<Span<'static>> = Vec::with_capacity(line.spans.len() + 1);
        spans.push(Span::raw("  "));
        spans.extend(line.spans);
        lines.push(Line::from(spans));
    }
    if truncated {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                TRUNCATION_HINT.to_string(),
                Style::new().add_modifier(Modifier::ITALIC),
            ),
        ]));
    }
    lines.push(Line::raw(""));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ApiRequestSide, ApiResponsePayload, ApiResponseSide};
    use serde_json::{Value, json};

    fn sample() -> Request {
        Request::complete(
            ApiResponsePayload {
                duration: Some(10.0),
                request: ApiRequestSide {
                    url: "https://x".into(),
                    method: Some("GET".into()),
                    data: crate::protocol::Body::null(),
                    headers: None,
                    params: None,
                },
                response: ApiResponseSide {
                    status: 200,
                    headers: None,
                    body: crate::protocol::Body::from_value(&json!({"ok": true})),
                },
            },
            None,
        )
    }

    #[test]
    fn pretty_body_string_prefers_response_when_populated() {
        let r = sample();
        let s = pretty_body_string(&r);
        assert!(s.contains("\"ok\""));
    }

    #[test]
    fn pretty_body_string_falls_back_to_request_body_if_response_is_empty() {
        let mut r = sample();
        r.exchange.response.body = crate::protocol::Body::null();
        r.exchange.request.data = crate::protocol::Body::from_value(&json!({"only": "in_req"}));
        let s = pretty_body_string(&r);
        assert!(s.contains("\"only\""));
    }

    // Silence unused-import warning now that Value is only used in
    // bodies above via the typed helpers.
    #[allow(dead_code)]
    fn _uses_value(v: &Value) -> &Value {
        v
    }
}
