//! Internal helpers shared by MCP tool implementations.
//!
//! These are pure functions operating on store rows — no I/O except for
//! the `scan_existing` helper that queries the store. Unit tests cover
//! filtering, matching, and summary rendering.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use uuid::Uuid;

use crate::bus::RequestId;
use crate::store::{Request, SecretsMode, StoreError, StoreHandle};

use super::schema::RequestSummary;

/// Parse a `RequestId` from the UUID string shape emitted by
/// [`RequestId::to_string`]. Returns `None` on malformed input.
pub(super) fn parse_request_id(s: &str) -> Option<RequestId> {
    Uuid::parse_str(s).ok().map(RequestId)
}

/// Compose the compact one-line summary object emitted by `list_requests`
/// and `search_requests`.
pub(super) fn summarise(r: Request) -> RequestSummary {
    RequestSummary {
        id: r.id.to_string(),
        time: format_hhmmss(r.received_at),
        method: r
            .exchange
            .request
            .method
            .clone()
            .unwrap_or_else(|| "???".to_string()),
        status: r.exchange.response.status,
        duration_ms: r.exchange.duration,
        url: r.exchange.request.url.clone(),
    }
}

/// Produce the full request/response detail blob that `get_request` and
/// `wait_for_request` return. Nested objects mirror the upstream
/// `api.response` payload verbatim so agents have all the context.
pub(super) fn detail_of(r: &Request) -> Value {
    json!({
        "id": r.id.to_string(),
        "time": format_hhmmss(r.received_at),
        "state": match r.state {
            crate::store::State::Complete => "complete",
            crate::store::State::Pending => "pending",
            crate::store::State::Orphaned => "orphaned",
        },
        "request": {
            "url": r.exchange.request.url,
            "method": r.exchange.request.method,
            "headers": r.exchange.request.headers,
            "params": r.exchange.request.params,
            "data": r.exchange.request.data,
        },
        "response": {
            "status": r.exchange.response.status,
            "headers": r.exchange.response.headers,
            "body": r.exchange.response.body,
        },
        "durationMs": r.exchange.duration,
    })
}

/// Evaluate the composed filter used by `list_requests`. All parameters
/// are optional and compose with logical AND.
pub(super) fn filter_matches(
    r: &Request,
    method: Option<&str>,
    status_class: Option<&str>,
    url_contains: Option<&str>,
) -> bool {
    if let Some(m) = method {
        let Some(got) = r.exchange.request.method.as_deref() else {
            return false;
        };
        if !got.eq_ignore_ascii_case(m) {
            return false;
        }
    }
    if let Some(cls) = status_class {
        if !status_class_matches(r.exchange.response.status, cls) {
            return false;
        }
    }
    if let Some(needle) = url_contains {
        if !r
            .exchange
            .request
            .url
            .to_ascii_lowercase()
            .contains(&needle.to_ascii_lowercase())
        {
            return false;
        }
    }
    true
}

/// Case-insensitive substring search across URL, request body, and
/// response body. Body storage is already compact JSON text (see
/// `protocol::Body`), so substring match against the stored text
/// catches field names, scalar values, and stringified JSON alike.
pub(super) fn text_search_matches(r: &Request, needle_lower: &str) -> bool {
    if r.exchange
        .request
        .url
        .to_ascii_lowercase()
        .contains(needle_lower)
    {
        return true;
    }
    if body_contains(&r.exchange.request.data, needle_lower) {
        return true;
    }
    if body_contains(&r.exchange.response.body, needle_lower) {
        return true;
    }
    false
}

fn body_contains(body: &crate::protocol::Body, needle_lower: &str) -> bool {
    if body.is_null() {
        return false;
    }
    // Stored compact JSON text already contains the structural form
    // we want to search — no re-serialisation per query.
    body.text().to_ascii_lowercase().contains(needle_lower)
}

#[expect(
    dead_code,
    reason = "kept for future use if we re-add raw-Value search paths"
)]
fn value_contains(v: &Value, needle_lower: &str) -> bool {
    if v.is_null() {
        return false;
    }
    if let Some(s) = v.as_str() {
        return s.to_ascii_lowercase().contains(needle_lower);
    }
    // Compact-serialize objects / arrays / numbers / bools so agent
    // queries like `"status":500` or field-name searches hit. Failure
    // to serialise is impossible for valid `Value`s, but we stay defensive.
    match serde_json::to_string(v) {
        Ok(s) => s.to_ascii_lowercase().contains(needle_lower),
        Err(_) => false,
    }
}

/// Predicate used by `wait_for_request` — URL substring + optional method.
/// `method` is expected uppercase because the caller normalises before
/// entering the wait loop.
pub(super) fn matches_wait(r: &Request, pattern_lower: &str, method_upper: Option<&str>) -> bool {
    if !r
        .exchange
        .request
        .url
        .to_ascii_lowercase()
        .contains(pattern_lower)
    {
        return false;
    }
    if let Some(m) = method_upper {
        let Some(got) = r.exchange.request.method.as_deref() else {
            return false;
        };
        if !got.eq_ignore_ascii_case(m) {
            return false;
        }
    }
    true
}

/// Look for a matching row already in the buffer. Used by
/// `wait_for_request` before and after a bus lag so we do not miss
/// events that arrived while we were rearming the subscription. `mode`
/// controls whether sensitive headers are masked in the returned
/// detail.
pub(super) async fn scan_existing(
    store: &StoreHandle,
    pattern_lower: &str,
    method_upper: Option<&str>,
    mode: SecretsMode,
) -> Result<Option<Value>, StoreError> {
    let rows = store.all(mode).await?;
    Ok(rows
        .into_iter()
        .rev()
        .find(|r| matches_wait(r, pattern_lower, method_upper))
        .map(|r| detail_of(&r)))
}

/// Return `true` when `status` matches `cls` (`"2xx"` / `"3xx"` / `"4xx"` /
/// `"5xx"` / `"pending"`). Unknown class strings match nothing.
fn status_class_matches(status: u16, cls: &str) -> bool {
    match cls.to_ascii_lowercase().as_str() {
        "2xx" => (200..300).contains(&status),
        "3xx" => (300..400).contains(&status),
        "4xx" => (400..500).contains(&status),
        "5xx" => (500..600).contains(&status),
        "pending" => false, // no v1 source of pending rows (ADR-003 §F-3)
        _ => false,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ApiRequestSide, ApiResponsePayload, ApiResponseSide};
    use serde_json::Value;

    fn row(method: &str, url: &str, status: u16, body_str: Option<&str>) -> Request {
        let body = match body_str {
            Some(s) => crate::protocol::Body::from_value(&Value::String(s.to_string())),
            None => crate::protocol::Body::null(),
        };
        let exchange = ApiResponsePayload {
            duration: Some(12.5),
            request: ApiRequestSide {
                url: url.to_string(),
                method: Some(method.to_string()),
                data: crate::protocol::Body::null(),
                headers: None,
                params: None,
            },
            response: ApiResponseSide {
                status,
                headers: None,
                body,
            },
        };
        Request::complete(exchange, None)
    }

    #[test]
    fn filter_matches_compose_logical_and() {
        let r = row("POST", "https://api.test/login", 200, None);
        assert!(filter_matches(&r, Some("post"), Some("2xx"), Some("login")));
        assert!(!filter_matches(&r, Some("get"), None, None));
        assert!(!filter_matches(&r, None, Some("4xx"), None));
        assert!(!filter_matches(&r, None, None, Some("logout")));
    }

    #[test]
    fn status_class_bucket_matches_correctly() {
        assert!(status_class_matches(204, "2xx"));
        assert!(status_class_matches(308, "3xx"));
        assert!(status_class_matches(418, "4xx"));
        assert!(status_class_matches(503, "5xx"));
        assert!(!status_class_matches(200, "5xx"));
        assert!(!status_class_matches(200, "pending"));
        assert!(!status_class_matches(200, "bogus"));
    }

    #[test]
    fn text_search_matches_url_and_string_bodies() {
        let r = row("GET", "https://api.test/feed", 200, Some("hello world"));
        assert!(text_search_matches(&r, "feed"));
        assert!(text_search_matches(&r, "hello"));
        assert!(!text_search_matches(&r, "banana"));
    }

    #[test]
    fn text_search_matches_object_body_fields_and_values() {
        // Agents often search for a field name or a scalar value inside
        // a structured JSON response — not a string body.
        let mut r = row("GET", "https://api.test/user", 200, None);
        r.exchange.response.body = crate::protocol::Body::from_value(&serde_json::json!({
            "userId": 42,
            "email": "jane@example.com",
            "tags": ["admin", "beta"],
        }));
        assert!(text_search_matches(&r, "userid"));
        assert!(text_search_matches(&r, "jane"));
        assert!(text_search_matches(&r, "admin"));
        assert!(text_search_matches(&r, "42"));
        assert!(!text_search_matches(&r, "banana"));
    }

    #[test]
    fn text_search_matches_array_and_number_bodies() {
        let mut r = row("POST", "https://api.test/nums", 200, None);
        r.exchange.request.data = crate::protocol::Body::from_value(&serde_json::json!([1, 2, 3]));
        r.exchange.response.body = crate::protocol::Body::from_value(&serde_json::json!(true));
        assert!(text_search_matches(&r, "[1,2,3]"));
        assert!(text_search_matches(&r, "true"));
    }

    #[test]
    fn matches_wait_respects_url_and_method() {
        let r = row("POST", "https://api.test/auth/login", 200, None);
        assert!(matches_wait(&r, "auth/login", Some("POST")));
        assert!(!matches_wait(&r, "auth/login", Some("GET")));
        assert!(matches_wait(&r, "auth/login", None));
    }

    #[test]
    fn parse_request_id_roundtrips() {
        let id = RequestId::new();
        let s = id.to_string();
        let parsed = parse_request_id(&s).unwrap();
        assert_eq!(parsed, id);
        assert!(parse_request_id("not-a-uuid").is_none());
    }

    #[test]
    fn summarise_produces_compact_shape() {
        let r = row("DELETE", "https://x/a", 204, None);
        let s = summarise(r.clone());
        assert_eq!(s.method, "DELETE");
        assert_eq!(s.status, 204);
        assert_eq!(s.url, "https://x/a");
        assert_eq!(s.id, r.id.to_string());
    }
}
