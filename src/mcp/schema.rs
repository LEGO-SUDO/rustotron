//! MCP tool parameter and response types.
//!
//! Each tool pairs a `Parameters<T>` input struct (with `JsonSchema`
//! derived so rmcp can publish the schema to clients) with a
//! `serde_json::Value` output assembled inside the tool body. Summaries
//! are deliberately narrow to keep token counts small (PRD AC:
//! `list_requests limit=10` < 2 KB).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Parameters for `list_requests`.
#[derive(Debug, Default, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListRequestsParams {
    /// Maximum rows to return. Default 20, hard cap 200.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Restrict to a single HTTP method (e.g. `"POST"`). Case-insensitive.
    #[serde(default)]
    pub method: Option<String>,
    /// Restrict to a single status class: `"2xx"`, `"3xx"`, `"4xx"`,
    /// `"5xx"`, or `"pending"`.
    #[serde(default)]
    pub status_class: Option<String>,
    /// Substring URL filter (case-insensitive).
    #[serde(default)]
    pub url_contains: Option<String>,
}

/// Parameters for `get_request`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GetRequestParams {
    /// The `RequestId` (UUID string) returned by `list_requests`.
    pub id: String,
    /// When `true`, returns unredacted headers. Default `false` —
    /// sensitive header values are masked with `***`.
    #[serde(default)]
    pub include_secrets: bool,
}

/// Parameters for `search_requests`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequestsParams {
    /// Case-insensitive substring; matches against URL, request body
    /// (when string), and response body (when string).
    pub query: String,
    /// Maximum rows to return. Default 20, hard cap 200.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Parameters for `wait_for_request`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WaitForRequestParams {
    /// URL substring (case-insensitive) to match. Optionally combined
    /// with `method`.
    pub url_pattern: String,
    /// Optional method restriction.
    #[serde(default)]
    pub method: Option<String>,
    /// How long to wait for a match, in milliseconds. Defaults to 30 s;
    /// hard cap 10 min to prevent runaway agent calls.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// When `true`, the returned detail keeps sensitive header values
    /// verbatim. Default `false` — sensitive headers are masked.
    /// Matches the `includeSecrets` semantics of `get_request`
    /// (PRD NFR-13).
    #[serde(default)]
    pub include_secrets: bool,
}

/// One line in a `list_requests` or `search_requests` response. Fields
/// are chosen to fit within a tweet-sized line and answer the questions
/// an agent asks most often.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestSummary {
    /// The `RequestId` string — pass this to `get_request` for details.
    pub id: String,
    /// ISO-ish `HH:MM:SS` when the row was received.
    pub time: String,
    /// HTTP method, or `"???"` if the client omitted one.
    pub method: String,
    /// HTTP status code.
    pub status: u16,
    /// Request duration in milliseconds, if known.
    pub duration_ms: Option<f64>,
    /// Full URL.
    pub url: String,
}

/// Default / maximum limits for list-style responses.
pub const DEFAULT_LIST_LIMIT: usize = 20;
/// Hard cap on how many rows any tool will return in one shot.
pub const MAX_LIST_LIMIT: usize = 200;
/// Default timeout for `wait_for_request` when the caller omits one.
pub const DEFAULT_WAIT_TIMEOUT_MS: u64 = 30_000;
/// Hard cap on `wait_for_request` timeout.
pub const MAX_WAIT_TIMEOUT_MS: u64 = 10 * 60 * 1_000;
