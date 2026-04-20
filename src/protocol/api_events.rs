//! `api.response` payload and the nested request/response sides.
//!
//! Reactotron's official React Native networking plugin emits a single
//! `api.response` frame per HTTP exchange that carries both the request and
//! response halves — see `docs/protocol.md` §5.3 and ADR-003 §F-1.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::body::Body;

/// A complete HTTP exchange as observed by the RN networking plugin.
///
/// All nullable fields (`method`, header maps, `duration`) are typed as
/// `Option` because the upstream source explicitly permits them to be
/// missing (ADR-003 §F-5). Bodies (`request.data`, `response.body`) are
/// stored as [`Body`] — compact JSON text capped at
/// [`super::MAX_STORED_BODY_BYTES`] so a multi-MB response can't blow
/// the ring buffer. Lazy-parsed back to a `Value` on display via
/// `body.as_value()`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiResponsePayload {
    /// Milliseconds between `XMLHttpRequest.send()` and the response
    /// arriving. `None` if the plugin's timer was never started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    /// The request half — URL, method, headers the app sent.
    pub request: ApiRequestSide,
    /// The response half — status code, headers, body.
    pub response: ApiResponseSide,
}

/// The "outbound" half of an HTTP exchange.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiRequestSide {
    /// Full URL, query string included.
    pub url: String,
    /// HTTP verb. Nullable per source (edge: non-XHR hooks set it to null).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// Body the app passed to `XMLHttpRequest.send()`. Often a JSON string,
    /// sometimes an object, occasionally a Reactotron sentinel.
    #[serde(default, skip_serializing_if = "Body::is_null")]
    pub data: Body,
    /// Request headers, if the plugin could read them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, String>>,
    /// Query params, parsed from the URL by the plugin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<HashMap<String, String>>,
}

/// The "inbound" half of an HTTP exchange.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiResponseSide {
    /// HTTP status code.
    pub status: u16,
    /// Response headers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, String>>,
    /// Response body — parsed JSON, raw string, or `"~~~ skipped ~~~"`.
    #[serde(default, skip_serializing_if = "Body::is_null")]
    pub body: Body,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn decodes_realistic_exchange() {
        let json = r#"{
            "duration": 142.7,
            "request": {
                "url": "https://api.example.com/login",
                "method": "POST",
                "data": "{\"email\":\"a@b\"}",
                "headers": {"Content-Type": "application/json"},
                "params": null
            },
            "response": {
                "status": 200,
                "headers": {"Content-Type": "application/json"},
                "body": {"token": "xyz"}
            }
        }"#;
        let payload: ApiResponsePayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.duration, Some(142.7));
        assert_eq!(payload.request.method.as_deref(), Some("POST"));
        assert_eq!(payload.response.status, 200);
        let body_value = payload.response.body.as_value().unwrap();
        assert_eq!(body_value.get("token").and_then(Value::as_str), Some("xyz"));
    }

    #[test]
    fn accepts_null_duration() {
        let json = r#"{
            "duration": null,
            "request": {"url": "https://x", "method": "GET"},
            "response": {"status": 500}
        }"#;
        let payload: ApiResponsePayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.duration, None);
    }

    #[test]
    fn accepts_skipped_body_sentinel() {
        let json = r#"{
            "duration": 10.0,
            "request": {"url": "https://x/logo.png", "method": "GET"},
            "response": {"status": 200, "body": "~~~ skipped ~~~"}
        }"#;
        let payload: ApiResponsePayload = serde_json::from_str(json).unwrap();
        assert_eq!(
            payload.response.body.as_string_literal().as_deref(),
            Some("~~~ skipped ~~~"),
            "sentinel strings pass through verbatim (ADR-003 §F-4)"
        );
    }

    #[test]
    fn accepts_missing_method() {
        let json = r#"{
            "duration": 1.0,
            "request": {"url": "https://x", "method": null},
            "response": {"status": 200}
        }"#;
        let payload: ApiResponsePayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.request.method, None);
    }
}
