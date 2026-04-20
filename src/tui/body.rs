//! Body rendering helpers.
//!
//! Pretty-prints a stored [`Body`] for the detail pane and truncates
//! oversize payloads so the render stays responsive. Separated from
//! `view::detail` so the logic is unit-testable without a ratatui
//! `Buffer`.
//!
//! Note: the *store* caps bodies at `MAX_STORED_BODY_BYTES` (256 KB) to
//! keep memory bounded. `DEFAULT_BODY_LIMIT` here is the *display*
//! limit — how much we'll actually render inside the detail pane before
//! hinting at the full-view modal.

use serde_json::Value;

use crate::protocol::Body;

/// One megabyte. Bodies larger than this get truncated in the detail
/// pane with a hint to open the full-view modal.
pub const DEFAULT_BODY_LIMIT: usize = 1_048_576;

/// Pretty-print a stored [`Body`] as a string. Handles null, stringified-JSON,
/// and truncated bodies cleanly.
#[must_use]
pub fn pretty_json_string(body: &Body) -> String {
    if body.is_null() {
        return "(empty)".to_string();
    }
    // Lazy parse: only now, on detail render, do we turn the stored
    // text into a Value tree. Freed after the string is built.
    match body.as_value() {
        Some(Value::String(s)) => {
            if let Ok(inner) = serde_json::from_str::<Value>(&s) {
                return pretty_json_string_value(&inner);
            }
            s
        }
        Some(other) => pretty_json_string_value(&other),
        None => {
            // Body was truncated or non-JSON — show the raw text (plus
            // truncation marker if present).
            body.as_pretty_string().into_owned()
        }
    }
}

fn pretty_json_string_value(v: &Value) -> String {
    if v.is_null() {
        return "(empty)".to_string();
    }
    if let Some(s) = v.as_str() {
        if let Ok(inner) = serde_json::from_str::<Value>(s) {
            return pretty_json_string_value(&inner);
        }
        return s.to_string();
    }
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

/// Truncate `body` to `limit` bytes if it is larger. Returns the
/// possibly-trimmed string and a `truncated` flag so the caller can
/// decide whether to emit a hint line.
///
/// Small bodies (≤ `limit`) pass through untouched.
#[must_use]
pub fn truncate_if_large(body: String, limit: usize) -> (String, bool) {
    if body.len() <= limit {
        return (body, false);
    }
    let mut out = body;
    // Find a valid UTF-8 boundary at or below `limit`.
    let mut cut = limit;
    while cut > 0 && !out.is_char_boundary(cut) {
        cut -= 1;
    }
    out.truncate(cut);
    (out, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_body_passes_through_untouched() {
        let s = "small body".to_string();
        let (out, truncated) = truncate_if_large(s.clone(), DEFAULT_BODY_LIMIT);
        assert_eq!(out, s);
        assert!(!truncated);
    }

    #[test]
    fn body_over_limit_is_truncated_and_flagged() {
        let big = "a".repeat(DEFAULT_BODY_LIMIT + 10);
        let (out, truncated) = truncate_if_large(big, DEFAULT_BODY_LIMIT);
        assert_eq!(out.len(), DEFAULT_BODY_LIMIT);
        assert!(truncated);
    }

    #[test]
    fn truncate_respects_utf8_boundary() {
        // Build a string where `limit` lands inside a multibyte char.
        let prefix = "a".repeat(1_048_574); // 1 MB - 2
        let s = format!("{prefix}日本"); // each char is 3 bytes
        let (out, truncated) = truncate_if_large(s, DEFAULT_BODY_LIMIT);
        assert!(truncated);
        // Result is a valid UTF-8 string.
        assert!(out.as_str().chars().count() > 0);
        assert!(out.len() <= DEFAULT_BODY_LIMIT);
    }

    #[test]
    fn pretty_json_string_handles_null() {
        assert_eq!(pretty_json_string(&Body::null()), "(empty)");
    }

    #[test]
    fn pretty_json_string_reparses_stringified_json() {
        let b = Body::from_value(&Value::String(r#"{"a":1}"#.to_string()));
        let out = pretty_json_string(&b);
        assert!(out.contains("\"a\""));
        assert!(out.contains("1"));
        assert!(out.contains('\n'));
    }

    #[test]
    fn pretty_json_string_preserves_plain_strings() {
        let b = Body::from_value(&Value::String("not json at all".to_string()));
        assert_eq!(pretty_json_string(&b), "not json at all");
    }
}
