//! Mock fixtures used by `rustotron --mock` to exercise the TUI without
//! a live RN client.
//!
//! Twenty requests with mixed verbs, status classes, path lengths, and
//! body sizes — enough to represent every visual branch the renderer
//! needs (status colouring, truncation, empty body).

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};

use crate::protocol::{ApiRequestSide, ApiResponsePayload, ApiResponseSide};
use crate::store::Request;

struct Spec {
    method: &'static str,
    url: &'static str,
    status: u16,
    duration_ms: f64,
    body: fn() -> Value,
}

fn empty_body() -> Value {
    Value::Null
}

fn small_body() -> Value {
    json!({ "ok": true, "id": "req_2A9" })
}

fn login_body() -> Value {
    json!({
        "token": "eyJhbGciOiJIUzI1NiIsInR5c...",
        "refresh": "r_8f2c9d1",
        "user": {
            "id": 4218,
            "email": "claude@example.com",
            "roles": ["admin", "viewer"]
        }
    })
}

fn medium_body() -> Value {
    json!({
        "page": 1,
        "pageSize": 20,
        "totalPages": 5,
        "items": (0..12)
            .map(|i| json!({
                "id": format!("item_{i:03}"),
                "name": format!("Mock item {i}"),
                "price_cents": 1000 + i * 37,
                "available": i % 3 != 0,
            }))
            .collect::<Vec<_>>(),
    })
}

fn error_body() -> Value {
    json!({
        "error": "invalid_token",
        "message": "The bearer token has expired. Refresh it and try again.",
        "hint": "POST /auth/refresh with refresh_token"
    })
}

fn server_error_body() -> Value {
    json!({
        "error": "internal_server_error",
        "trace_id": "abc-123-def-456",
    })
}

/// Full spec table — 20 rows by construction.
const SPECS: &[Spec] = &[
    Spec {
        method: "POST",
        url: "https://api.example.com/auth/login",
        status: 200,
        duration_ms: 142.0,
        body: login_body,
    },
    Spec {
        method: "GET",
        url: "https://api.example.com/users/me",
        status: 200,
        duration_ms: 48.0,
        body: small_body,
    },
    Spec {
        method: "GET",
        url: "https://api.example.com/feed?cursor=2A9",
        status: 200,
        duration_ms: 211.0,
        body: medium_body,
    },
    Spec {
        method: "GET",
        url: "https://api.example.com/notifications/unread",
        status: 304,
        duration_ms: 28.0,
        body: empty_body,
    },
    Spec {
        method: "PUT",
        url: "https://api.example.com/profile",
        status: 200,
        duration_ms: 95.0,
        body: small_body,
    },
    Spec {
        method: "GET",
        url: "https://api.example.com/missing-resource",
        status: 404,
        duration_ms: 31.0,
        body: error_body,
    },
    Spec {
        method: "POST",
        url: "https://api.example.com/payments/charges",
        status: 201,
        duration_ms: 788.0,
        body: medium_body,
    },
    Spec {
        method: "DELETE",
        url: "https://api.example.com/sessions/old-id",
        status: 204,
        duration_ms: 17.0,
        body: empty_body,
    },
    Spec {
        method: "GET",
        url: "https://cdn.example.com/assets/images/logo.png",
        status: 200,
        duration_ms: 62.0,
        body: empty_body,
    },
    Spec {
        method: "POST",
        url: "https://api.example.com/search?q=reactotron",
        status: 200,
        duration_ms: 356.0,
        body: medium_body,
    },
    Spec {
        method: "PATCH",
        url: "https://api.example.com/settings/preferences",
        status: 200,
        duration_ms: 71.0,
        body: small_body,
    },
    Spec {
        method: "GET",
        url: "https://api.example.com/orders/2345/items",
        status: 401,
        duration_ms: 14.0,
        body: error_body,
    },
    Spec {
        method: "GET",
        url: "https://analytics.example.com/events/track",
        status: 202,
        duration_ms: 9.0,
        body: empty_body,
    },
    Spec {
        method: "POST",
        url: "https://api.example.com/v2/batch/ingest",
        status: 500,
        duration_ms: 1042.0,
        body: server_error_body,
    },
    Spec {
        method: "GET",
        url: "https://api.example.com/really/long/url/that/does/not/fit/in/the/pane/easily",
        status: 200,
        duration_ms: 118.0,
        body: small_body,
    },
    Spec {
        method: "POST",
        url: "https://api.example.com/upload/blob",
        status: 413,
        duration_ms: 26.0,
        body: error_body,
    },
    Spec {
        method: "GET",
        url: "https://api.example.com/healthz",
        status: 200,
        duration_ms: 4.0,
        body: small_body,
    },
    Spec {
        method: "GET",
        url: "https://api.example.com/flags",
        status: 429,
        duration_ms: 8.0,
        body: error_body,
    },
    Spec {
        method: "DELETE",
        url: "https://api.example.com/cache/keys/user_4218",
        status: 200,
        duration_ms: 19.0,
        body: small_body,
    },
    Spec {
        method: "POST",
        url: "https://api.example.com/reports/generate",
        status: 503,
        duration_ms: 2150.0,
        body: server_error_body,
    },
];

/// Produce 20 pre-baked [`ApiResponsePayload`]s. Timestamps are relative
/// to the deterministic anchor so snapshot tests can pin them.
#[must_use]
pub fn mock_exchanges() -> Vec<ApiResponsePayload> {
    SPECS
        .iter()
        .enumerate()
        .map(|(i, s)| build_exchange(i, s))
        .collect()
}

fn build_exchange(i: usize, spec: &Spec) -> ApiResponsePayload {
    let mut req_headers: HashMap<String, String> = HashMap::new();
    req_headers.insert("Content-Type".to_string(), "application/json".to_string());
    req_headers.insert("Accept".to_string(), "application/json".to_string());
    // Put a secret on one row so the redaction path gets exercised from
    // the TUI side too.
    if spec.url.contains("/auth/login") {
        req_headers.insert(
            "Authorization".to_string(),
            "Bearer super-secret-XYZ".to_string(),
        );
    }

    let mut resp_headers: HashMap<String, String> = HashMap::new();
    resp_headers.insert(
        "Content-Type".to_string(),
        "application/json; charset=utf-8".to_string(),
    );
    resp_headers.insert("X-Request-Id".to_string(), format!("req-{i:04}"));

    ApiResponsePayload {
        duration: Some(spec.duration_ms),
        request: ApiRequestSide {
            url: spec.url.to_string(),
            method: Some(spec.method.to_string()),
            data: crate::protocol::Body::from_value(&(spec.body)()),
            headers: Some(req_headers),
            params: None,
        },
        response: ApiResponseSide {
            status: spec.status,
            headers: Some(resp_headers),
            body: crate::protocol::Body::from_value(&(spec.body)()),
        },
    }
}

/// Produce 20 deterministic `Request` rows. The `received_at` timestamps
/// march forward at 1 second intervals starting from `anchor` so the
/// list order matches the insert order and snapshot tests get stable
/// HH:MM:SS columns.
#[must_use]
pub fn mock_rows(anchor: SystemTime) -> Vec<Request> {
    mock_exchanges()
        .into_iter()
        .enumerate()
        .map(|(i, ex)| {
            let when = anchor + Duration::from_secs(i as u64);
            let mut r = Request::complete(ex, None);
            r.received_at = when;
            r
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_has_twenty_rows() {
        assert_eq!(mock_exchanges().len(), 20);
    }

    #[test]
    fn mock_covers_every_status_class() {
        let classes: std::collections::HashSet<u16> =
            SPECS.iter().map(|s| s.status / 100).collect();
        // 2xx, 3xx, 4xx, 5xx.
        for class in [2, 3, 4, 5] {
            assert!(
                classes.contains(&class),
                "mock should exercise {class}xx class"
            );
        }
    }

    #[test]
    fn mock_covers_common_verbs() {
        let methods: std::collections::HashSet<&str> = SPECS.iter().map(|s| s.method).collect();
        for m in ["GET", "POST", "PUT", "DELETE", "PATCH"] {
            assert!(methods.contains(m), "mock should include {m}");
        }
    }
}
