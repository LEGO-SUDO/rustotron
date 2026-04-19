//! Snapshot tests for the TUI view layer.
//!
//! Uses `ratatui::backend::TestBackend` to render into an in-memory buffer
//! at a fixed 120×40 size, then asserts on a stable text rendering via
//! `insta`. `TERM` and `NO_COLOR` are pinned in the test harness to keep
//! snapshots reproducible across hosts.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use insta::assert_snapshot;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::json;

use rustotron::protocol::{ApiRequestSide, ApiResponsePayload, ApiResponseSide};
use rustotron::store::Request;
use rustotron::tui::app::{App, SectionId};
use rustotron::tui::event::PaneId;
use rustotron::tui::filter::StatusClass;
use rustotron::tui::mock;
use rustotron::tui::theme::Theme;
use rustotron::tui::view;

const COLS: u16 = 120;
const ROWS: u16 = 40;

/// Deterministic anchor so mock timestamps render identically across runs.
fn anchor() -> SystemTime {
    // 2026-04-19 12:00:00 UTC — arbitrary but stable.
    UNIX_EPOCH + Duration::from_secs(1_776_772_800)
}

fn plain_render(app: &mut App) -> String {
    // Safety: tests run single-threaded per binary by default and we only
    // set process-scoped env for deterministic snapshots. `set_var` is
    // marked unsafe in edition 2024 because of thread-safety concerns; we
    // accept that here and document it.
    unsafe {
        std::env::set_var("TERM", "xterm-256color");
        std::env::set_var("NO_COLOR", "1");
    }
    let theme = Theme::plain();
    let backend = TestBackend::new(COLS, ROWS);
    let mut terminal = Terminal::new(backend).expect("TestBackend creation never fails");
    terminal
        .draw(|frame| view::draw(frame, app, &theme))
        .expect("TestBackend draw never fails");
    buffer_to_string(terminal.backend().buffer())
}

fn buffer_to_string(buf: &ratatui::buffer::Buffer) -> String {
    let area = buf.area;
    let mut out = String::with_capacity((area.width as usize + 1) * area.height as usize);
    for y in 0..area.height {
        for x in 0..area.width {
            let cell = &buf[(x, y)];
            out.push_str(cell.symbol());
        }
        // Trim trailing spaces per row so snapshots stay compact and
        // don't flap on widget padding changes.
        while out.ends_with(' ') {
            out.pop();
        }
        out.push('\n');
    }
    out
}

fn build_app_with_large_body() -> App {
    // One giant body row > 1 MB so the truncation hint shows up.
    let payload = ApiResponsePayload {
        duration: Some(5.0),
        request: ApiRequestSide {
            url: "https://api.example.com/large".to_string(),
            method: Some("GET".to_string()),
            data: serde_json::Value::Null,
            headers: None,
            params: None,
        },
        response: ApiResponseSide {
            status: 200,
            headers: None,
            body: json!({ "giant": "x".repeat(1_200_000) }),
        },
    };
    let mut req = Request::complete(payload, None);
    req.received_at = anchor();
    let mut app = App::new("ws://127.0.0.1:9091", true);
    app.set_rows(vec![req]);
    app.focus(PaneId::Detail);
    app
}

#[test]
fn snapshot_empty_state() {
    let mut app = App::new("ws://127.0.0.1:9091", false);
    let rendered = plain_render(&mut app);
    assert_snapshot!("tui_empty_state", rendered);
}

#[test]
fn snapshot_list_view_with_mock_data() {
    let mut app = App::new("ws://127.0.0.1:9091", true);
    app.set_rows(mock::mock_rows(anchor()));
    let rendered = plain_render(&mut app);
    assert_snapshot!("tui_list_view_mock", rendered);
}

#[test]
fn snapshot_detail_view_focused() {
    let mut app = App::new("ws://127.0.0.1:9091", true);
    app.set_rows(mock::mock_rows(anchor()));
    app.focus(PaneId::Detail);
    let rendered = plain_render(&mut app);
    assert_snapshot!("tui_detail_view_focused", rendered);
}

#[test]
fn snapshot_detail_view_with_request_body_collapsed() {
    let mut app = App::new("ws://127.0.0.1:9091", true);
    app.set_rows(mock::mock_rows(anchor()));
    app.focus(PaneId::Detail);
    app.toggle_section(SectionId::RequestBody);
    let rendered = plain_render(&mut app);
    assert_snapshot!("tui_detail_collapsed_request_body", rendered);
}

#[test]
fn snapshot_detail_view_large_body_truncated() {
    let mut app = build_app_with_large_body();
    let rendered = plain_render(&mut app);
    assert_snapshot!("tui_detail_truncated_body", rendered);
}

#[test]
fn snapshot_filter_bar_input_open() {
    let mut app = App::new("ws://127.0.0.1:9091", true);
    app.set_rows(mock::mock_rows(anchor()));
    app.begin_filter_input();
    for c in "search".chars() {
        app.push_filter_char(c);
    }
    let rendered = plain_render(&mut app);
    assert_snapshot!("tui_filter_input_open", rendered);
}

#[test]
fn snapshot_filter_bar_method_active_post() {
    let mut app = App::new("ws://127.0.0.1:9091", true);
    app.set_rows(mock::mock_rows(anchor()));
    app.toggle_method("POST");
    let rendered = plain_render(&mut app);
    assert_snapshot!("tui_filter_method_post_active", rendered);
}

#[test]
fn snapshot_filter_bar_status_class_active() {
    let mut app = App::new("ws://127.0.0.1:9091", true);
    app.set_rows(mock::mock_rows(anchor()));
    app.toggle_status_class(StatusClass::ClientError);
    let rendered = plain_render(&mut app);
    assert_snapshot!("tui_filter_status_4xx_active", rendered);
}

#[test]
fn snapshot_confirm_clear_modal() {
    let mut app = App::new("ws://127.0.0.1:9091", true);
    app.set_rows(mock::mock_rows(anchor()));
    app.begin_clear_confirm();
    let rendered = plain_render(&mut app);
    assert_snapshot!("tui_confirm_clear_modal", rendered);
}
