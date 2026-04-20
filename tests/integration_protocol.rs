//! Fixture replay — every captured `.ndjson` in
//! `tests/fixtures/reactotron-traces/` must decode cleanly with the codec
//! producing the variants each file's name implies.
//!
//! These fixtures are the wire-level contract the codec owes. If they drift
//! (e.g. a new RN version adds a field), update the fixture + the codec in
//! the same PR.

use rustotron::protocol::{Message, decode};
use std::fs;
use std::path::PathBuf;

fn fixtures_dir() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/reactotron-traces");
    p
}

fn load(name: &str) -> Vec<String> {
    let path = fixtures_dir().join(name);
    let body = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read fixture {}: {e}", path.display()));
    body.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.to_owned())
        .collect()
}

fn decode_all(name: &str) -> Vec<Message> {
    load(name)
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            decode(&line).unwrap_or_else(|e| panic!("{name} line {}: decode error: {e}", i + 1))
        })
        .collect()
}

#[test]
fn handshake_ok_decodes_to_intro_then_set_client_id() {
    let msgs = decode_all("handshake-ok.ndjson");
    assert_eq!(msgs.len(), 2);
    match &msgs[0] {
        Message::ClientIntro(p) => {
            assert_eq!(p.name, "DemoApp");
            assert_eq!(p.environment.as_deref(), Some("development"));
            assert!(
                p.client_id.is_none(),
                "intro without clientId triggers server reply"
            );
            assert_eq!(
                p.extra.get("platform").and_then(|v| v.as_str()),
                Some("ios")
            );
        }
        other => panic!("expected ClientIntro, got {other:?}"),
    }
    match &msgs[1] {
        Message::SetClientId(guid) => {
            assert!(
                !guid.is_empty(),
                "setClientId payload must be a non-empty guid"
            );
        }
        other => panic!("expected SetClientId, got {other:?}"),
    }
}

#[test]
fn api_request_response_pair_decodes_to_intro_then_complete_exchange() {
    let msgs = decode_all("api-request-response-pair.ndjson");
    assert_eq!(msgs.len(), 2);
    match &msgs[0] {
        Message::ClientIntro(p) => {
            assert_eq!(
                p.client_id.as_deref(),
                Some("7f3c1e2a-9b4d-4c5f-8a6e-1d2c3b4a5f60")
            );
        }
        other => panic!("expected ClientIntro, got {other:?}"),
    }
    match &msgs[1] {
        Message::ApiResponse(p) => {
            assert_eq!(p.request.url, "https://api.example.com/api/login?retry=1");
            assert_eq!(p.request.method.as_deref(), Some("POST"));
            assert_eq!(p.response.status, 200);
            assert_eq!(p.duration, Some(142.7));
            let body = p.response.body.as_value().expect("body parses");
            assert_eq!(
                body.get("token")
                    .and_then(|v| v.as_str())
                    .map(|s| s.starts_with("eyJ")),
                Some(true)
            );
        }
        other => panic!("expected ApiResponse, got {other:?}"),
    }
}

#[test]
fn orphaned_response_decodes_without_error() {
    // Named "orphaned" for forward-compat with plugins that split events.
    // Upstream RN plugin never produces this shape (ADR-003 §F-1), but the
    // fixture must still decode cleanly so the codec tolerates it.
    let msgs = decode_all("api-response-orphaned.ndjson");
    assert!(!msgs.is_empty(), "fixture must contain at least one frame");
}

#[test]
fn pending_fixture_routes_hypothetical_api_request_to_unknown() {
    // The `api.request` type is not in our v1 codec's known list (ADR-003
    // §F-1). Fixtures that contain it must decode as Unknown — NOT error.
    let msgs = decode_all("api-request-pending.ndjson");
    let has_unknown_for_api_request = msgs.iter().any(|m| matches!(m, Message::Unknown(v) if v.get("type").and_then(|t| t.as_str()) == Some("api.request")));
    assert!(
        has_unknown_for_api_request,
        "api.request type should decode to Message::Unknown — forward-compat for 3rd-party plugins"
    );
}

#[test]
fn mixed_unknown_types_decode_without_error() {
    let msgs = decode_all("mixed-with-unknown-types.ndjson");
    assert!(msgs.len() >= 5, "fixture should contain a variety of types");

    // Count variant distribution so we can assert the realistic mix.
    let mut intros = 0usize;
    let mut set_ids = 0usize;
    let mut api_responses = 0usize;
    let mut unknowns = 0usize;
    for m in &msgs {
        match m {
            Message::ClientIntro(_) => intros += 1,
            Message::SetClientId(_) => set_ids += 1,
            Message::ApiResponse(_) => api_responses += 1,
            Message::Unknown(_) => unknowns += 1,
        }
    }
    assert_eq!(intros, 1, "one client.intro per fixture");
    assert_eq!(set_ids, 1, "one setClientId per fixture");
    assert!(
        api_responses >= 2,
        "fixture simulates multiple api responses"
    );
    assert!(
        unknowns >= 3,
        "fixture includes log / display / state / benchmark / unknown-plugin frames — all Unknown"
    );
}

#[test]
fn mixed_unknown_preserves_unknown_plugin_types_verbatim() {
    // `plugin.unknown.future` is a fabricated type specifically to test
    // forward-compat: new plugins we have never seen must roundtrip through
    // Message::Unknown with the full envelope preserved.
    let msgs = decode_all("mixed-with-unknown-types.ndjson");
    let unknown_plugin = msgs
        .iter()
        .find(|m| {
            matches!(
                m,
                Message::Unknown(v) if v.get("type").and_then(|t| t.as_str()) == Some("plugin.unknown.future")
            )
        })
        .expect("plugin.unknown.future frame should be present and decode to Unknown");
    match unknown_plugin {
        Message::Unknown(v) => {
            assert_eq!(
                v.get("payload")
                    .and_then(|p| p.get("hello"))
                    .and_then(|h| h.as_str()),
                Some("from a plugin we have never seen"),
                "Unknown must preserve the original payload verbatim for debug inspection"
            );
        }
        _ => unreachable!(),
    }
}
