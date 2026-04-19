//! Top-level `Message` enum, `Envelope` shape, and the encode/decode codec
//! that converts between them and raw JSON frames.
//!
//! See `docs/protocol.md` and ADR-003 for the full spec. Short version:
//!
//! - Every wire frame is one JSON object with `{ type, payload?, important?,
//!   date?, deltaTime? }`.
//! - `type` is the discriminant. Known values: `client.intro`,
//!   `setClientId`, `api.response`. Everything else → `Message::Unknown`.
//! - A frame whose `type` is known but whose payload fails to parse is NOT
//!   a hard error — we fall through to `Message::Unknown` and log at `warn`.
//!   A single malformed frame must never kill the session (NFR-9).

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::api_events::ApiResponsePayload;
use super::handshake::ClientIntroPayload;
use super::repair;

/// Every decodable message rustotron cares about in v1. Unknown types
/// (`log`, `state.*`, custom plugin events, …) land in `Unknown` along with
/// the full envelope so logging / debugging can inspect them later.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// `type == "client.intro"`. First frame the RN client sends after
    /// the WS handshake. Payload carries the app name, optional clientId,
    /// and an open-ended block of environment info.
    ClientIntro(ClientIntroPayload),
    /// `type == "setClientId"`. Server → client only. Sent when the
    /// incoming `client.intro` had no `clientId`. Payload is a single
    /// GUID string.
    SetClientId(String),
    /// `type == "api.response"`. A completed HTTP exchange. Carries both
    /// request and response halves — Reactotron's networking plugin never
    /// emits a separate `api.request` frame (ADR-003 §F-1).
    ApiResponse(ApiResponsePayload),
    /// Any other `type`, plus any frame whose payload failed to match the
    /// expected shape for its known `type` (graceful degradation per
    /// NFR-9). Carries the full envelope for debugging.
    Unknown(Value),
}

/// The shared wire envelope. Client-originated frames always carry all five
/// fields; server-originated `setClientId` is encoded hand-built (without
/// `date` / `deltaTime`) to mirror upstream byte shape — see ADR-003 §F-7.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope<P> {
    /// Discriminant — always matches the variant's wire name.
    #[serde(rename = "type")]
    pub ty: String,
    /// Payload — absent for types like `clear` that carry no data.
    #[serde(default = "default_payload", skip_serializing_if = "Option::is_none")]
    pub payload: Option<P>,
    /// "Highlight in UI" flag — informational, always false for rustotron.
    #[serde(default)]
    pub important: bool,
    /// ISO-8601 client-side timestamp. Kept as a string; rustotron uses
    /// server-receipt time for ordering (per `docs/protocol.md` §8).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub date: String,
    /// Ms since the client's previous outbound message. Can be `0`.
    #[serde(default, rename = "deltaTime")]
    pub delta_time: f64,
}

fn default_payload<P>() -> Option<P> {
    None
}

/// Errors the codec can return on encode / decode paths. All other failure
/// modes (known-type with bad payload, unknown type) degrade to
/// `Message::Unknown` instead of erroring.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    /// The frame body was not syntactically valid JSON.
    #[error("invalid JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
    /// Frame decoded to JSON but the top-level was not an object. Reactotron
    /// always sends an object.
    #[error("wire envelope is not a JSON object")]
    NotAnObject,
    /// `type` field was missing or not a string.
    #[error("envelope missing string 'type' field")]
    MissingType,
}

/// Encode a `Message` to a JSON string suitable for writing to a WebSocket
/// text frame.
///
/// # Errors
///
/// Returns `CodecError::InvalidJson` only if serde_json fails to serialise
/// an otherwise-valid payload (effectively unreachable for our payload
/// types, but bubbled for completeness).
pub fn encode(msg: &Message) -> Result<String, CodecError> {
    let value = match msg {
        Message::ClientIntro(payload) => json!({
            "type": "client.intro",
            "payload": payload,
            "important": false,
            "date": "",
            "deltaTime": 0.0,
        }),
        Message::SetClientId(guid) => json!({
            "type": "setClientId",
            "payload": guid,
        }),
        Message::ApiResponse(payload) => json!({
            "type": "api.response",
            "payload": payload,
            "important": false,
            "date": "",
            "deltaTime": 0.0,
        }),
        Message::Unknown(value) => value.clone(),
    };
    Ok(serde_json::to_string(&value)?)
}

/// Decode one WebSocket text frame into a `Message`.
///
/// Graceful-degradation policy (NFR-9):
///
/// - Invalid JSON / non-object / missing `type` → `Err(CodecError)`.
/// - Unknown `type` → `Ok(Message::Unknown(full_envelope))`.
/// - Known `type` with malformed payload → `Ok(Message::Unknown(full_envelope))`
///   plus a `tracing::warn!` so the bad frame is visible without killing
///   the session.
///
/// # Errors
///
/// See policy above — only JSON-level failures produce `Err`.
pub fn decode(frame: &str) -> Result<Message, CodecError> {
    let root: Value = serde_json::from_str(frame)?;
    let obj = root.as_object().ok_or(CodecError::NotAnObject)?;
    let ty = obj
        .get("type")
        .and_then(Value::as_str)
        .ok_or(CodecError::MissingType)?;

    // Clone the payload once; re-used when we fall back to Unknown so the
    // trace context is preserved for whoever inspects the log / debug surface.
    // Apply Reactotron's falsy-value repair pass *before* strict type
    // deserialisation (see `super::repair` for the rationale — fields like
    // `response.headers` routinely arrive as the string `"~~~ null ~~~"`).
    let mut payload = obj.get("payload").cloned().unwrap_or(Value::Null);
    repair::repair(&mut payload);

    match ty {
        "client.intro" => match serde_json::from_value::<ClientIntroPayload>(payload) {
            Ok(p) => Ok(Message::ClientIntro(p)),
            Err(e) => {
                tracing::warn!(error = %e, "client.intro payload failed to parse; falling back to Unknown");
                Ok(Message::Unknown(root))
            }
        },
        "setClientId" => match obj.get("payload").and_then(Value::as_str) {
            Some(guid) => Ok(Message::SetClientId(guid.to_owned())),
            None => {
                tracing::warn!(
                    "setClientId payload missing or non-string; falling back to Unknown"
                );
                Ok(Message::Unknown(root))
            }
        },
        "api.response" => match serde_json::from_value::<ApiResponsePayload>(payload) {
            Ok(p) => Ok(Message::ApiResponse(p)),
            Err(e) => {
                tracing::warn!(error = %e, "api.response payload failed to parse; falling back to Unknown");
                Ok(Message::Unknown(root))
            }
        },
        _ => Ok(Message::Unknown(root)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn decodes_client_intro() {
        let frame = r#"{
            "type": "client.intro",
            "payload": {"name": "DemoApp", "clientId": "abc"},
            "important": false,
            "date": "2026-04-19T12:00:00.000Z",
            "deltaTime": 0
        }"#;
        let msg = decode(frame).unwrap();
        match msg {
            Message::ClientIntro(p) => {
                assert_eq!(p.name, "DemoApp");
                assert_eq!(p.client_id.as_deref(), Some("abc"));
            }
            other => panic!("expected ClientIntro, got {other:?}"),
        }
    }

    #[test]
    fn decodes_set_client_id_slim_envelope() {
        let frame = r#"{"type":"setClientId","payload":"guid-123"}"#;
        match decode(frame).unwrap() {
            Message::SetClientId(g) => assert_eq!(g, "guid-123"),
            other => panic!("expected SetClientId, got {other:?}"),
        }
    }

    #[test]
    fn decodes_api_response() {
        let frame = r#"{
            "type": "api.response",
            "payload": {
                "duration": 100.0,
                "request": {"url": "https://x", "method": "GET"},
                "response": {"status": 200}
            },
            "important": false,
            "date": "2026-04-19T12:00:00.000Z",
            "deltaTime": 10
        }"#;
        match decode(frame).unwrap() {
            Message::ApiResponse(p) => {
                assert_eq!(p.duration, Some(100.0));
                assert_eq!(p.request.url, "https://x");
                assert_eq!(p.response.status, 200);
            }
            other => panic!("expected ApiResponse, got {other:?}"),
        }
    }

    #[test]
    fn unknown_type_falls_through_to_unknown_variant() {
        let frame = r#"{
            "type": "log",
            "payload": {"level": "debug", "message": "hi"},
            "date": "2026-04-19T12:00:00.000Z",
            "deltaTime": 0
        }"#;
        match decode(frame).unwrap() {
            Message::Unknown(v) => {
                assert_eq!(v.get("type").and_then(Value::as_str), Some("log"));
                assert_eq!(
                    v.get("payload")
                        .and_then(|p| p.get("level"))
                        .and_then(Value::as_str),
                    Some("debug")
                );
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn known_type_with_bad_payload_falls_through_to_unknown_not_err() {
        // api.response without the required `request` / `response` objects.
        let frame = r#"{"type":"api.response","payload":{"duration":10.0}}"#;
        match decode(frame).unwrap() {
            Message::Unknown(v) => {
                assert_eq!(v.get("type").and_then(Value::as_str), Some("api.response"));
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn invalid_json_returns_err() {
        let frame = "not-json";
        let result = decode(frame);
        assert!(matches!(result, Err(CodecError::InvalidJson(_))));
    }

    #[test]
    fn non_object_root_returns_err() {
        let frame = r#"[1,2,3]"#;
        let result = decode(frame);
        assert!(matches!(result, Err(CodecError::NotAnObject)));
    }

    #[test]
    fn missing_type_returns_err() {
        let frame = r#"{"payload": 42}"#;
        let result = decode(frame);
        assert!(matches!(result, Err(CodecError::MissingType)));
    }

    #[test]
    fn encode_set_client_id_uses_slim_envelope() {
        let msg = Message::SetClientId("guid-x".into());
        let s = encode(&msg).unwrap();
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v.get("type").and_then(Value::as_str), Some("setClientId"));
        assert_eq!(v.get("payload").and_then(Value::as_str), Some("guid-x"));
        assert!(v.get("date").is_none(), "setClientId must not include date");
        assert!(
            v.get("deltaTime").is_none(),
            "setClientId must not include deltaTime"
        );
    }

    #[test]
    fn encode_client_intro_roundtrips() {
        let frame = r#"{
            "type": "client.intro",
            "payload": {"name": "DemoApp", "clientId": "abc"},
            "important": false,
            "date": "2026-04-19T12:00:00.000Z",
            "deltaTime": 0
        }"#;
        let decoded = decode(frame).unwrap();
        let re_encoded = encode(&decoded).unwrap();
        let re_decoded = decode(&re_encoded).unwrap();
        assert_eq!(decoded, re_decoded);
    }

    #[test]
    fn api_response_with_null_sentinel_headers_decodes_successfully() {
        // Regression: Reactotron's RN networking plugin emits
        // `"~~~ null ~~~"` for response.headers when the fetch completes
        // without exposed headers. Before the repair pass, this whole
        // frame was lost to `Message::Unknown`.
        let frame = r#"{
            "type": "api.response",
            "payload": {
                "duration": 42.0,
                "request": {
                    "url": "https://api.example.com/ping",
                    "method": "GET",
                    "headers": "~~~ null ~~~",
                    "params": "~~~ null ~~~"
                },
                "response": {
                    "status": 200,
                    "headers": "~~~ null ~~~",
                    "body": "pong"
                }
            },
            "important": false,
            "date": "2026-04-19T12:00:00.000Z",
            "deltaTime": 0
        }"#;
        match decode(frame).unwrap() {
            Message::ApiResponse(p) => {
                assert_eq!(p.duration, Some(42.0));
                assert_eq!(p.request.method.as_deref(), Some("GET"));
                assert!(p.request.headers.is_none(), "null-sentinel → None");
                assert!(p.request.params.is_none());
                assert!(p.response.headers.is_none());
                assert_eq!(p.response.body.as_str(), Some("pong"));
            }
            other => panic!("expected ApiResponse, got {other:?}"),
        }
    }

    #[test]
    fn unknown_round_trip_preserves_full_envelope() {
        let frame = r#"{"type":"log","payload":{"level":"warn"},"date":"now","deltaTime":1}"#;
        let decoded = decode(frame).unwrap();
        let re_encoded = encode(&decoded).unwrap();
        let re_decoded = decode(&re_encoded).unwrap();
        assert_eq!(decoded, re_decoded);
    }
}
