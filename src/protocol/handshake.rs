//! `client.intro` payload and the slim `setClientId` server→client envelope.
//!
//! Source: `docs/protocol.md` §5.1, §5.2 and ADR-003 §F-6, §F-7.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Payload carried by the first frame a Reactotron client sends after the
/// WebSocket upgrade completes.
///
/// Only `name` is formally required by the upstream contract. RN clients
/// always send a long `client.*` block (platform, model, screen size, etc.)
/// that grows with each RN release, so we capture all non-modelled fields in
/// `extra` rather than enumerating a schema that would drift.
///
/// See ADR-003 §F-6.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientIntroPayload {
    /// The RN app's display name. Upstream contract marks this required;
    /// rustotron surfaces it in the connection status bar.
    pub name: String,
    /// Optional client-side identifier. When absent, rustotron replies with
    /// a `setClientId` frame carrying a generated UUID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// `"development"` / `"production"` / custom. Informational.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    /// Everything else (`platform`, `platformVersion`, `model`, `screenWidth`,
    /// `reactNativeVersion`, …) flows through here. The TUI and MCP pluck
    /// specific keys on demand.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialises_minimal_intro() {
        let json = r#"{"name":"DemoApp"}"#;
        let intro: ClientIntroPayload = serde_json::from_str(json).unwrap();
        assert_eq!(intro.name, "DemoApp");
        assert_eq!(intro.client_id, None);
        assert!(intro.extra.is_empty());
    }

    #[test]
    fn captures_unknown_fields_in_extra() {
        let json = r#"{
            "name": "DemoApp",
            "environment": "development",
            "clientId": "abc-123",
            "platform": "ios",
            "platformVersion": "17.4",
            "screenWidth": 390
        }"#;
        let intro: ClientIntroPayload = serde_json::from_str(json).unwrap();
        assert_eq!(intro.name, "DemoApp");
        assert_eq!(intro.environment.as_deref(), Some("development"));
        assert_eq!(intro.client_id.as_deref(), Some("abc-123"));
        assert_eq!(
            intro.extra.get("platform").and_then(Value::as_str),
            Some("ios")
        );
        assert_eq!(
            intro.extra.get("screenWidth").and_then(Value::as_i64),
            Some(390)
        );
    }

    #[test]
    fn roundtrip_preserves_extra_fields() {
        let json = r#"{"name":"App","clientId":"id","platform":"android"}"#;
        let intro: ClientIntroPayload = serde_json::from_str(json).unwrap();
        let re = serde_json::to_value(&intro).unwrap();
        assert_eq!(re.get("name").and_then(Value::as_str), Some("App"));
        assert_eq!(re.get("clientId").and_then(Value::as_str), Some("id"));
        assert_eq!(re.get("platform").and_then(Value::as_str), Some("android"));
    }
}
