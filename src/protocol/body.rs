//! Compact, lazy-parsed wrapper around an API request/response body.
//!
//! **Why this exists.** Reactotron frames arrive with bodies already
//! modelled as `serde_json::Value`. In memory, a parsed `Value` is 5–10×
//! the size of its compact JSON text (per-enum discriminators, boxed
//! strings, `BTreeMap` overhead per nested object). For a wallet-style
//! app pulling portfolio / NFT metadata, that adds up fast — a 100-row
//! buffer can reach hundreds of MB.
//!
//! This module:
//!
//! 1. Serialises the incoming `Value` once into compact JSON text.
//! 2. Stores the text (not the tree).
//! 3. Caps the stored text at [`MAX_STORED_BODY_BYTES`] so a single
//!    multi-MB response can't blow the buffer.
//! 4. Parses back to `Value` on demand (`as_value`), only when a view
//!    actually needs the tree — detail pane, cURL export, MCP detail.
//!
//! The compact-JSON text is also the perfect representation for MCP
//! substring search (no re-serialisation per query) and for the cURL
//! exporter's `-d '…'` flag, so most consumers don't need to re-parse.

use std::borrow::Cow;

use serde::de::{self, Deserialize, Deserializer};
use serde::ser::{Serialize, Serializer};
use serde_json::Value;

/// Hard cap on how many bytes of body text we'll retain per row.
/// 256 KB is well above any normal API response; multi-MB JSON blobs get
/// truncated with an inline marker so the buffer stays bounded.
pub const MAX_STORED_BODY_BYTES: usize = 256 * 1024;

/// Inline marker appended to truncated text. Visible in the detail pane
/// and in cURL exports so users know something was cut.
const TRUNCATION_MARKER: &str = "…[truncated]";

/// Stored body — a compact JSON text plus the original size in bytes.
/// Cheap to clone (Arc-backed? No — `String`; see note in [`crate::store::request::Request`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Body {
    /// Compact JSON text. May include a trailing [`TRUNCATION_MARKER`]
    /// if the original exceeded [`MAX_STORED_BODY_BYTES`].
    text: String,
    /// Number of bytes the original payload serialised to. When this
    /// is greater than `text.len()`, the body was truncated.
    original_bytes: u64,
}

impl Body {
    /// Build an empty (JSON `null`) body.
    #[must_use]
    pub fn null() -> Self {
        Self {
            text: "null".to_string(),
            original_bytes: 4,
        }
    }

    /// Build a body from a freshly-parsed `serde_json::Value`. Serialises
    /// compactly; caps at [`MAX_STORED_BODY_BYTES`].
    #[must_use]
    pub fn from_value(v: &Value) -> Self {
        // Fast path: null → a single owned string literal.
        if v.is_null() {
            return Self::null();
        }
        let raw = serde_json::to_string(v).unwrap_or_else(|_| "null".to_string());
        Self::from_owned_text(raw)
    }

    /// Build a body directly from a string already known to be compact
    /// JSON text (or any UTF-8 payload we want to store verbatim).
    /// Caller owns the allocation; we truncate in place if needed.
    #[must_use]
    pub fn from_owned_text(mut text: String) -> Self {
        let original_bytes = text.len() as u64;
        if text.len() > MAX_STORED_BODY_BYTES {
            truncate_at_char_boundary(&mut text, MAX_STORED_BODY_BYTES);
            text.push_str(TRUNCATION_MARKER);
        }
        Self {
            text,
            original_bytes,
        }
    }

    /// Raw JSON text as stored. Never empty; null bodies are the
    /// literal string `"null"`.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Bytes the original payload serialised to, before truncation.
    /// Equal to `text().len()` when the body wasn't truncated.
    #[must_use]
    pub fn original_bytes(&self) -> u64 {
        self.original_bytes
    }

    /// True if the stored text is shorter than the original payload.
    #[must_use]
    pub fn is_truncated(&self) -> bool {
        self.original_bytes as usize > self.text.len()
    }

    /// True if this body is the JSON null literal. Used by callers that
    /// want to skip rendering an empty section.
    #[must_use]
    pub fn is_null(&self) -> bool {
        self.text == "null"
    }

    /// Parse the stored text back into a `Value`. Returns `None` if the
    /// body was truncated (and the partial text is no longer valid JSON)
    /// or if the text was never valid JSON to begin with.
    ///
    /// Callers that just want a display string should use
    /// [`as_pretty_string`](Self::as_pretty_string) — it gracefully
    /// handles truncated / non-JSON payloads.
    #[must_use]
    pub fn as_value(&self) -> Option<Value> {
        serde_json::from_str(&self.text).ok()
    }

    /// Return a display-ready pretty-printed string. Falls back to the
    /// raw stored text when the body can't be parsed (truncated / was
    /// never JSON). Never allocates for null.
    #[must_use]
    pub fn as_pretty_string(&self) -> Cow<'_, str> {
        if self.is_null() {
            return Cow::Borrowed("null");
        }
        match self.as_value() {
            Some(v) => match serde_json::to_string_pretty(&v) {
                Ok(s) => Cow::Owned(s),
                Err(_) => Cow::Borrowed(&self.text),
            },
            None => Cow::Borrowed(&self.text),
        }
    }

    /// If the body is a JSON string literal (e.g. `"~~~ skipped ~~~"`
    /// or a stringified-JSON blob), return the inner string.
    #[must_use]
    pub fn as_string_literal(&self) -> Option<String> {
        if !self.text.starts_with('"') || !self.text.ends_with('"') || self.text.len() < 2 {
            return None;
        }
        serde_json::from_str::<String>(&self.text).ok()
    }

    /// Length of the stored text in bytes. Not the original size — use
    /// [`original_bytes`](Self::original_bytes) for that.
    #[must_use]
    pub fn stored_len(&self) -> usize {
        self.text.len()
    }
}

impl Default for Body {
    fn default() -> Self {
        Self::null()
    }
}

impl<'de> Deserialize<'de> for Body {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Parse through Value so serde's tree model handles arbitrary
        // input (object, array, number, bool, string, null), then
        // immediately compact-serialise and cap. We throw the Value
        // away after this point — the whole point of Body is to avoid
        // carrying it around.
        let v = Value::deserialize(deserializer)?;
        if v.is_null() {
            return Ok(Self::null());
        }
        let text = serde_json::to_string(&v).map_err(de::Error::custom)?;
        Ok(Self::from_owned_text(text))
    }
}

impl Serialize for Body {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Emit as real JSON if the stored text parses; else emit as a
        // JSON string (covers truncated payloads and non-JSON strings
        // like the "~~~ skipped ~~~" sentinel once it's been stripped
        // of quotes). Parsing on every serialisation is acceptable —
        // this is called from detail rendering / MCP responses, not
        // from the hot path.
        match serde_json::from_str::<Value>(&self.text) {
            Ok(v) => v.serialize(serializer),
            Err(_) => self.text.serialize(serializer),
        }
    }
}

/// Trim a `String` to at most `max_bytes` without splitting a UTF-8
/// scalar. Scans backward from `max_bytes` to find the nearest char
/// boundary — O(up-to-3) work.
fn truncate_at_char_boundary(s: &mut String, max_bytes: usize) {
    let mut cut = max_bytes.min(s.len());
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s.truncate(cut);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn null_value_stores_as_null_literal() {
        let b = Body::from_value(&Value::Null);
        assert!(b.is_null());
        assert_eq!(b.text(), "null");
        assert!(!b.is_truncated());
    }

    #[test]
    fn object_value_round_trips_through_compact_text() {
        let v = json!({"ok": true, "n": 42});
        let b = Body::from_value(&v);
        // Key order on serialize depends on serde_json's map impl
        // (BTreeMap by default → alphabetical). Assert structure, not
        // byte order.
        let parsed = b.as_value().expect("parses");
        assert_eq!(parsed, v);
        assert!(!b.is_truncated());
    }

    #[test]
    fn large_body_is_truncated_with_marker() {
        let mut big = String::with_capacity(MAX_STORED_BODY_BYTES * 2);
        big.push('"');
        for _ in 0..(MAX_STORED_BODY_BYTES * 2) {
            big.push('x');
        }
        big.push('"');
        let original_len = big.len();
        let b = Body::from_owned_text(big);
        assert!(b.is_truncated());
        assert_eq!(b.original_bytes(), original_len as u64);
        assert!(b.text().ends_with(TRUNCATION_MARKER));
        // Truncated text won't parse back to JSON — that's expected.
        assert!(b.as_value().is_none());
        // …but display still works via as_pretty_string fallback.
        assert!(b.as_pretty_string().ends_with(TRUNCATION_MARKER));
    }

    #[test]
    fn truncation_respects_utf8_char_boundary() {
        let mut long = String::new();
        // Fill with a 3-byte char so boundary-alignment matters.
        while long.len() < MAX_STORED_BODY_BYTES + 100 {
            long.push('€'); // 3 bytes
        }
        let b = Body::from_owned_text(long);
        assert!(b.is_truncated());
        // Simply parsing the text must not panic; truncation is byte-valid UTF-8.
        let _ = b.text().len();
        // The head before the marker is valid UTF-8 too.
        let head = &b.text()[..b.text().len() - TRUNCATION_MARKER.len()];
        assert!(std::str::from_utf8(head.as_bytes()).is_ok());
    }

    #[test]
    fn deserializes_from_value_and_caps() {
        // Build a giant object and deserialise.
        let mut big = serde_json::Map::new();
        for i in 0..50_000u32 {
            big.insert(format!("key-{i}"), Value::String("v".repeat(10)));
        }
        let value = Value::Object(big);
        let json_text = serde_json::to_string(&value).unwrap();
        let b: Body = serde_json::from_str(&json_text).unwrap();
        assert!(b.is_truncated(), "expected cap to kick in");
        assert!(b.stored_len() <= MAX_STORED_BODY_BYTES + TRUNCATION_MARKER.len());
    }

    #[test]
    fn serialize_round_trips_small_value() {
        let v = json!({"a": [1, 2, 3]});
        let b = Body::from_value(&v);
        let ser = serde_json::to_value(&b).unwrap();
        assert_eq!(ser, v);
    }

    #[test]
    fn serialize_truncated_body_falls_back_to_string() {
        let big = "x".repeat(MAX_STORED_BODY_BYTES * 2);
        let b = Body::from_owned_text(big);
        // Truncated text isn't valid JSON, so serialization emits it
        // as a JSON string rather than raw JSON.
        let ser = serde_json::to_value(&b).unwrap();
        assert!(ser.is_string());
        assert!(ser.as_str().unwrap().ends_with(TRUNCATION_MARKER));
    }

    #[test]
    fn as_string_literal_extracts_inner_for_string_bodies() {
        let b = Body::from_value(&Value::String("hello".to_string()));
        assert_eq!(b.as_string_literal().as_deref(), Some("hello"));
        let obj = Body::from_value(&json!({"x": 1}));
        assert!(obj.as_string_literal().is_none());
    }

    #[test]
    fn pretty_string_for_object_indents() {
        let b = Body::from_value(&json!({"a": 1}));
        let pretty = b.as_pretty_string();
        assert!(pretty.contains('\n'), "pretty output should have newlines");
    }
}
