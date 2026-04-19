//! Header redaction for sensitive values.
//!
//! Rustotron stores the raw exchange. Redaction happens on the **read**
//! side, parameterised by [`SecretsMode`](super::request::SecretsMode).
//! The user's casual screenshot of the TUI must not leak an
//! `Authorization` bearer token (PRD US-5, NFR-12).
//!
//! The default sensitive-header list comes from PRD FR-5. Users can
//! extend it via config (`RUSTOTRON_REDACT_HEADERS`, `config.toml`).

use std::collections::HashMap;

use super::request::{Request, SecretsMode};

/// String written in place of any sensitive header value.
pub const REDACTION_MASK: &str = "***";

/// Default case-insensitive header names rustotron considers sensitive.
/// Callers usually extend this with user-supplied extras before passing
/// to [`apply_secrets_mode`].
#[must_use]
pub fn default_sensitive_headers() -> Vec<String> {
    vec![
        "authorization".to_string(),
        "cookie".to_string(),
        "set-cookie".to_string(),
        "x-api-key".to_string(),
        "proxy-authorization".to_string(),
    ]
}

/// Return a view of `req` with header maps rewritten according to `mode`.
///
/// `Raw` clones the input unchanged. `Redacted` replaces the value of any
/// header whose name (case-insensitively) matches `sensitive` with
/// [`REDACTION_MASK`].
///
/// Body redaction is deliberately **not** done here — bodies are free-form
/// JSON and PRD FR-5 scopes redaction to headers. If user reports show
/// tokens leaking via `body` (Set-Cookie duplicated into body, etc.) we
/// add body scrubbing in a later task.
#[must_use]
pub fn apply_secrets_mode(req: &Request, mode: SecretsMode, sensitive: &[String]) -> Request {
    match mode {
        SecretsMode::Raw => req.clone(),
        SecretsMode::Redacted => {
            let mut cloned = req.clone();
            if let Some(h) = cloned.exchange.request.headers.as_mut() {
                redact_header_map(h, sensitive);
            }
            if let Some(h) = cloned.exchange.response.headers.as_mut() {
                redact_header_map(h, sensitive);
            }
            cloned
        }
    }
}

fn redact_header_map(headers: &mut HashMap<String, String>, sensitive: &[String]) {
    for (k, v) in headers.iter_mut() {
        if sensitive.iter().any(|s| s.eq_ignore_ascii_case(k.as_str())) {
            *v = REDACTION_MASK.to_string();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ApiRequestSide, ApiResponsePayload, ApiResponseSide};
    use serde_json::Value;

    fn sample_request_with_headers(
        req_headers: HashMap<String, String>,
        res_headers: HashMap<String, String>,
    ) -> Request {
        let exchange = ApiResponsePayload {
            duration: Some(10.0),
            request: ApiRequestSide {
                url: "https://x".to_string(),
                method: Some("GET".to_string()),
                data: Value::Null,
                headers: Some(req_headers),
                params: None,
            },
            response: ApiResponseSide {
                status: 200,
                headers: Some(res_headers),
                body: Value::Null,
            },
        };
        Request::complete(exchange, None)
    }

    fn hdrs(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn raw_mode_returns_unchanged_headers() {
        let req = sample_request_with_headers(
            hdrs(&[("Authorization", "Bearer xyz")]),
            hdrs(&[("Set-Cookie", "session=abc")]),
        );
        let out = apply_secrets_mode(&req, SecretsMode::Raw, &default_sensitive_headers());
        assert_eq!(
            out.exchange
                .request
                .headers
                .as_ref()
                .and_then(|h| h.get("Authorization"))
                .map(String::as_str),
            Some("Bearer xyz")
        );
        assert_eq!(
            out.exchange
                .response
                .headers
                .as_ref()
                .and_then(|h| h.get("Set-Cookie"))
                .map(String::as_str),
            Some("session=abc")
        );
    }

    #[test]
    fn redacted_mode_masks_default_sensitive_headers_case_insensitively() {
        let req = sample_request_with_headers(
            hdrs(&[
                ("Authorization", "Bearer xyz"),
                ("Accept", "application/json"),
            ]),
            hdrs(&[
                ("Set-Cookie", "session=abc"),
                ("Content-Type", "text/plain"),
            ]),
        );
        let out = apply_secrets_mode(&req, SecretsMode::Redacted, &default_sensitive_headers());

        // Sensitive — masked.
        assert_eq!(
            out.exchange
                .request
                .headers
                .as_ref()
                .and_then(|h| h.get("Authorization"))
                .map(String::as_str),
            Some(REDACTION_MASK)
        );
        assert_eq!(
            out.exchange
                .response
                .headers
                .as_ref()
                .and_then(|h| h.get("Set-Cookie"))
                .map(String::as_str),
            Some(REDACTION_MASK)
        );

        // Non-sensitive — preserved.
        assert_eq!(
            out.exchange
                .request
                .headers
                .as_ref()
                .and_then(|h| h.get("Accept"))
                .map(String::as_str),
            Some("application/json")
        );
        assert_eq!(
            out.exchange
                .response
                .headers
                .as_ref()
                .and_then(|h| h.get("Content-Type"))
                .map(String::as_str),
            Some("text/plain")
        );
    }

    #[test]
    fn redacted_mode_honors_user_extra_sensitive_list() {
        let req = sample_request_with_headers(
            hdrs(&[
                ("X-Company-Token", "secret-1"),
                ("Accept", "application/json"),
            ]),
            HashMap::new(),
        );
        let mut sensitive = default_sensitive_headers();
        sensitive.push("x-company-token".to_string());
        let out = apply_secrets_mode(&req, SecretsMode::Redacted, &sensitive);
        assert_eq!(
            out.exchange
                .request
                .headers
                .as_ref()
                .and_then(|h| h.get("X-Company-Token"))
                .map(String::as_str),
            Some(REDACTION_MASK)
        );
        assert_eq!(
            out.exchange
                .request
                .headers
                .as_ref()
                .and_then(|h| h.get("Accept"))
                .map(String::as_str),
            Some("application/json"),
            "non-sensitive headers unaffected when extras are configured"
        );
    }

    #[test]
    fn redacted_mode_with_none_headers_is_a_noop() {
        let req = Request::complete(
            ApiResponsePayload {
                duration: Some(10.0),
                request: ApiRequestSide {
                    url: "https://x".to_string(),
                    method: Some("GET".to_string()),
                    data: Value::Null,
                    headers: None,
                    params: None,
                },
                response: ApiResponseSide {
                    status: 204,
                    headers: None,
                    body: Value::Null,
                },
            },
            None,
        );
        let out = apply_secrets_mode(&req, SecretsMode::Redacted, &default_sensitive_headers());
        assert!(out.exchange.request.headers.is_none());
        assert!(out.exchange.response.headers.is_none());
    }
}
