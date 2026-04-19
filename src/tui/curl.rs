//! cURL export.
//!
//! Builds a shell-ready `curl` command for a stored [`Request`] and copies
//! it to the system clipboard via `arboard`. Two modes: `Redacted` masks
//! sensitive headers (Authorization, Cookie, Set-Cookie, X-API-Key); `Raw`
//! includes their actual values. The redacted form is the default — the
//! raw form requires an explicit user action (`Y` / shift+y).
//!
//! Headless environments (CI, over SSH without DISPLAY, etc.) frequently
//! have no clipboard. In that case `copy_to_clipboard` returns a
//! [`CurlExportError::Clipboard`] and the caller is expected to log the
//! failure to stderr rather than panic — PRD NFR-10.

use serde_json::Value;

use crate::store::Request;

/// Whether to include sensitive header values verbatim.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SecretsMode {
    /// Mask Authorization / Cookie / Set-Cookie / X-API-Key to `<redacted>`.
    Redacted,
    /// Copy the raw header values as stored.
    Raw,
}

/// Errors surfaced by the clipboard export path. `copy_to_clipboard`
/// returns this; the caller converts to a friendly stderr line.
#[derive(Debug, thiserror::Error)]
pub enum CurlExportError {
    /// The OS clipboard is unavailable (headless CI, sandbox, no DISPLAY).
    #[error("clipboard unavailable: {0}")]
    Clipboard(String),
}

/// Build a multi-line `curl` command for the given stored request.
///
/// `sensitive` is the configured list of header names to redact in
/// `SecretsMode::Redacted`. Pass the merged list from `StoreConfig`
/// (defaults + user extras) rather than relying on a hardcoded default —
/// without that, custom headers like `X-Company-Token` leak (M-8).
///
/// Format:
///
/// ```text
/// curl -X POST 'https://api.example.com/login' \
///   -H 'Content-Type: application/json' \
///   -H 'Authorization: <redacted>' \
///   -d '{"email":"jane@example.com"}'
/// ```
///
/// Header order is deterministic (sorted lexicographically). The body
/// flag is omitted for requests without one. Single quotes inside values
/// are escaped as `'\''`.
#[must_use]
pub fn curl_command(req: &Request, mode: SecretsMode, sensitive: &[String]) -> String {
    let method = req
        .exchange
        .request
        .method
        .as_deref()
        .unwrap_or("GET")
        .to_uppercase();
    let url = &req.exchange.request.url;

    let mut out = String::new();
    out.push_str("curl -X ");
    out.push_str(&method);
    out.push_str(" '");
    out.push_str(&shell_escape(url));
    out.push('\'');

    if let Some(h) = req.exchange.request.headers.as_ref() {
        let mut keys: Vec<&String> = h.keys().collect();
        keys.sort();
        for k in keys {
            let raw = h.get(k).cloned().unwrap_or_default();
            let display_value = match mode {
                SecretsMode::Raw => raw,
                SecretsMode::Redacted => {
                    if is_sensitive(k, sensitive) {
                        "<redacted>".to_string()
                    } else {
                        raw
                    }
                }
            };
            out.push_str(" \\\n  -H '");
            out.push_str(&shell_escape(k));
            out.push_str(": ");
            out.push_str(&shell_escape(&display_value));
            out.push('\'');
        }
    }

    if let Some(body_str) = body_as_data_string(&req.exchange.request.data) {
        out.push_str(" \\\n  -d '");
        out.push_str(&shell_escape(&body_str));
        out.push('\'');
    }

    out
}

/// Copy the rendered cURL command to the system clipboard.
///
/// # Errors
///
/// Returns [`CurlExportError::Clipboard`] when the platform clipboard
/// cannot be opened (headless CI, sandbox, no DISPLAY server).
pub fn copy_to_clipboard(text: &str) -> Result<(), CurlExportError> {
    let mut cb =
        arboard::Clipboard::new().map_err(|e| CurlExportError::Clipboard(e.to_string()))?;
    cb.set_text(text.to_string())
        .map_err(|e| CurlExportError::Clipboard(e.to_string()))
}

fn is_sensitive(name: &str, sensitive: &[String]) -> bool {
    sensitive.iter().any(|s| s.eq_ignore_ascii_case(name))
}

/// Quote-safe body string. Returns `None` for empty / null bodies. For
/// JSON objects/arrays/numbers emits a compact JSON string; for embedded
/// strings (the common RN case — stringified JSON) uses the string
/// directly; for other scalars prints the `Display` form.
fn body_as_data_string(v: &Value) -> Option<String> {
    if v.is_null() {
        return None;
    }
    if let Some(s) = v.as_str() {
        if s.is_empty() {
            return None;
        }
        return Some(s.to_string());
    }
    serde_json::to_string(v).ok()
}

/// Single-quote escape for bash/zsh/fish. Replaces each `'` with `'\''`
/// so the resulting string is safe to paste between single quotes.
fn shell_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ApiRequestSide, ApiResponsePayload, ApiResponseSide};
    use crate::store::default_sensitive_headers;
    use serde_json::json;
    use std::collections::HashMap;

    fn default_sensitive() -> Vec<String> {
        default_sensitive_headers()
    }

    fn sample(url: &str, method: &str) -> Request {
        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        headers.insert("Authorization".to_string(), "Bearer real-token".to_string());
        headers.insert("X-Trace".to_string(), "abc".to_string());

        let exchange = ApiResponsePayload {
            duration: Some(10.0),
            request: ApiRequestSide {
                url: url.to_string(),
                method: Some(method.to_string()),
                data: json!({"email": "jane@example.com"}),
                headers: Some(headers),
                params: None,
            },
            response: ApiResponseSide {
                status: 200,
                headers: None,
                body: Value::Null,
            },
        };
        Request::complete(exchange, None)
    }

    #[test]
    fn redacted_masks_authorization_and_keeps_other_headers() {
        let req = sample("https://api.example.com/login", "POST");
        let cmd = curl_command(&req, SecretsMode::Redacted, &default_sensitive());
        assert!(cmd.contains("curl -X POST 'https://api.example.com/login'"));
        assert!(cmd.contains("-H 'Authorization: <redacted>'"));
        assert!(cmd.contains("-H 'Content-Type: application/json'"));
        assert!(cmd.contains("-H 'X-Trace: abc'"));
        assert!(cmd.contains("-d '{\"email\":\"jane@example.com\"}'"));
        assert!(!cmd.contains("Bearer real-token"));
    }

    #[test]
    fn raw_mode_includes_real_header_values() {
        let req = sample("https://api.example.com/login", "POST");
        let cmd = curl_command(&req, SecretsMode::Raw, &default_sensitive());
        assert!(cmd.contains("-H 'Authorization: Bearer real-token'"));
        assert!(!cmd.contains("<redacted>"));
    }

    #[test]
    fn get_without_body_omits_data_flag() {
        let mut req = sample("https://x", "GET");
        req.exchange.request.data = Value::Null;
        let cmd = curl_command(&req, SecretsMode::Redacted, &default_sensitive());
        assert!(!cmd.contains("-d '"));
    }

    #[test]
    fn stringified_json_body_uses_the_string_as_data() {
        let mut req = sample("https://x", "POST");
        req.exchange.request.data = Value::String(r#"{"a":1}"#.to_string());
        let cmd = curl_command(&req, SecretsMode::Redacted, &default_sensitive());
        assert!(cmd.contains(r#"-d '{"a":1}'"#));
    }

    #[test]
    fn header_order_is_deterministic() {
        let req = sample("https://x", "POST");
        let cmd = curl_command(&req, SecretsMode::Redacted, &default_sensitive());
        let auth_pos = cmd.find("Authorization").unwrap();
        let ct_pos = cmd.find("Content-Type").unwrap();
        let xt_pos = cmd.find("X-Trace").unwrap();
        // Sorted: Authorization < Content-Type < X-Trace.
        assert!(auth_pos < ct_pos);
        assert!(ct_pos < xt_pos);
    }

    #[test]
    fn single_quote_in_url_or_value_is_escaped() {
        let mut req = sample("https://api.example.com/o'hare", "GET");
        req.exchange.request.data = Value::Null;
        let cmd = curl_command(&req, SecretsMode::Redacted, &default_sensitive());
        assert!(cmd.contains("o'\\''hare"));
    }

    #[test]
    fn missing_method_defaults_to_get() {
        let mut req = sample("https://x", "GET");
        req.exchange.request.method = None;
        req.exchange.request.data = Value::Null;
        let cmd = curl_command(&req, SecretsMode::Redacted, &default_sensitive());
        assert!(cmd.starts_with("curl -X GET "));
    }
}
