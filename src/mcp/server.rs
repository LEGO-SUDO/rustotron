//! `RustotronMcp` — the MCP service handler exposed on stdio.
//!
//! Built on the `rmcp` tool-router pattern. Holds handles to the shared
//! store and event bus so tools can query current state and subscribe
//! to live events (`wait_for_request`). No mutable state lives here —
//! the store actor remains the single authority.

use std::time::Duration;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, Content, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo,
};
use rmcp::{ErrorData as McpError, ServerHandler, tool, tool_handler, tool_router};
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use crate::bus::{Event, EventBus};
use crate::store::{SecretsMode, StoreHandle};

use super::schema::{
    DEFAULT_LIST_LIMIT, DEFAULT_WAIT_TIMEOUT_MS, GetRequestParams, ListRequestsParams,
    MAX_LIST_LIMIT, MAX_WAIT_TIMEOUT_MS, RequestSummary, SearchRequestsParams,
    WaitForRequestParams,
};
use super::tools;

/// Service handed to `rmcp::ServiceExt::serve`. Cheap-to-clone; each
/// incoming tool call invokes the handler on a clone of the service.
#[derive(Clone)]
pub struct RustotronMcp {
    store: StoreHandle,
    bus: EventBus,
    // Populated by `Self::tool_router()` (a method synthesised by the
    // `#[tool_router]` macro). The router itself is consumed by the
    // `#[tool_handler]` impl below — clippy can't see that through the
    // macro expansion, hence `expect(dead_code)`.
    #[expect(dead_code, reason = "consumed via #[tool_handler] macro")]
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl RustotronMcp {
    /// Construct the service. `store` and `bus` must outlive the service
    /// (both are `Clone` and inexpensive to clone, so pass clones).
    #[must_use]
    pub fn new(store: StoreHandle, bus: EventBus) -> Self {
        Self {
            store,
            bus,
            tool_router: Self::tool_router(),
        }
    }

    /// Liveness probe. Returns version info and ring-buffer stats so
    /// agents can sanity-check before issuing other tools.
    #[tool(
        description = "Check that the rustotron MCP server is alive. Returns version and store stats."
    )]
    async fn health(&self) -> Result<CallToolResult, McpError> {
        let stats = self
            .store
            .stats()
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let out = json!({
            "ok": true,
            "version": env!("CARGO_PKG_VERSION"),
            "buffered": stats.0,
            "capacity": stats.1,
        });
        Ok(CallToolResult::success(vec![Content::text(
            out.to_string(),
        )]))
    }

    /// List recent requests with optional filters. Returns compact summaries
    /// (one object per row). Call `get_request` for the full detail of a row.
    #[tool(
        description = "List recent HTTP requests captured from the RN app. Filters compose; unspecified filters match everything. Returns compact summaries — call get_request for full detail."
    )]
    async fn list_requests(
        &self,
        Parameters(params): Parameters<ListRequestsParams>,
    ) -> Result<CallToolResult, McpError> {
        let rows = self
            .store
            .all(SecretsMode::Redacted)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let limit = params
            .limit
            .unwrap_or(DEFAULT_LIST_LIMIT)
            .min(MAX_LIST_LIMIT);

        let filtered: Vec<RequestSummary> = rows
            .into_iter()
            .rev()
            .filter(|r| {
                tools::filter_matches(
                    r,
                    params.method.as_deref(),
                    params.status_class.as_deref(),
                    params.url_contains.as_deref(),
                )
            })
            .take(limit)
            .map(tools::summarise)
            .collect();

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&filtered).unwrap_or_else(|_| "[]".to_string()),
        )]))
    }

    /// Get full detail for a single request.
    #[tool(
        description = "Get the full request and response for a single captured exchange, by RequestId. Sensitive headers are masked unless includeSecrets=true."
    )]
    async fn get_request(
        &self,
        Parameters(params): Parameters<GetRequestParams>,
    ) -> Result<CallToolResult, McpError> {
        let id = tools::parse_request_id(&params.id)
            .ok_or_else(|| McpError::invalid_params("invalid request id", None))?;
        let mode = if params.include_secrets {
            SecretsMode::Raw
        } else {
            SecretsMode::Redacted
        };
        let row = self
            .store
            .get(id, mode)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        match row {
            Some(r) => {
                let detail = tools::detail_of(&r);
                Ok(CallToolResult::success(vec![Content::text(
                    detail.to_string(),
                )]))
            }
            None => Err(McpError::invalid_params(
                format!("no request with id {}", params.id),
                None,
            )),
        }
    }

    /// Substring search across URL + bodies. Case-insensitive.
    #[tool(
        description = "Case-insensitive substring search across URL, request body (if string), and response body (if string). Returns compact summaries."
    )]
    async fn search_requests(
        &self,
        Parameters(params): Parameters<SearchRequestsParams>,
    ) -> Result<CallToolResult, McpError> {
        let rows = self
            .store
            .all(SecretsMode::Redacted)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let limit = params
            .limit
            .unwrap_or(DEFAULT_LIST_LIMIT)
            .min(MAX_LIST_LIMIT);

        let q = params.query.to_ascii_lowercase();
        let matches: Vec<RequestSummary> = rows
            .into_iter()
            .rev()
            .filter(|r| tools::text_search_matches(r, &q))
            .take(limit)
            .map(tools::summarise)
            .collect();

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&matches).unwrap_or_else(|_| "[]".to_string()),
        )]))
    }

    /// Block until a request matching `urlPattern` (substring, case-insensitive)
    /// arrives, or `timeoutMs` elapses. The killer tool for agent-driven
    /// debug flows — "wait for the POST to /auth/login, then confirm 200".
    #[tool(
        description = "Wait (up to timeoutMs) for an incoming request whose URL contains urlPattern. On match, returns the full detail of the matching exchange. On timeout, returns an error."
    )]
    async fn wait_for_request(
        &self,
        Parameters(params): Parameters<WaitForRequestParams>,
    ) -> Result<CallToolResult, McpError> {
        let pattern = params.url_pattern.to_ascii_lowercase();
        let method = params.method.map(|m| m.to_ascii_uppercase());
        let timeout_ms = params
            .timeout_ms
            .unwrap_or(DEFAULT_WAIT_TIMEOUT_MS)
            .min(MAX_WAIT_TIMEOUT_MS);
        let mode = if params.include_secrets {
            SecretsMode::Raw
        } else {
            SecretsMode::Redacted
        };

        // First: check if a matching row is already in the buffer. Agents
        // often query slightly after the request has already landed.
        if let Some(match_detail) =
            tools::scan_existing(&self.store, &pattern, method.as_deref(), mode)
                .await
                .map_err(|e| McpError::internal_error(e.to_string(), None))?
        {
            return Ok(CallToolResult::success(vec![Content::text(
                match_detail.to_string(),
            )]));
        }

        // Otherwise: subscribe and wait. Bus lagged errors restart the wait
        // from a fresh subscription — the store-scan above handles the
        // overlap so we don't miss events that landed between queries.
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
        let mut rx = self.bus.subscribe();
        loop {
            let remaining = deadline
                .checked_duration_since(tokio::time::Instant::now())
                .unwrap_or_default();
            if remaining.is_zero() {
                return Err(McpError::internal_error(
                    "timeout waiting for matching request".to_string(),
                    None,
                ));
            }
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok(Event::ResponseReceived(id))) => {
                    if let Some(row) = self
                        .store
                        .get(id, mode)
                        .await
                        .map_err(|e| McpError::internal_error(e.to_string(), None))?
                    {
                        if tools::matches_wait(&row, &pattern, method.as_deref()) {
                            return Ok(CallToolResult::success(vec![Content::text(
                                tools::detail_of(&row).to_string(),
                            )]));
                        }
                    }
                }
                Ok(Ok(_)) => continue,
                Ok(Err(RecvError::Lagged(_))) => {
                    // Rescan store for anything we might have missed.
                    if let Some(detail) =
                        tools::scan_existing(&self.store, &pattern, method.as_deref(), mode)
                            .await
                            .map_err(|e| McpError::internal_error(e.to_string(), None))?
                    {
                        return Ok(CallToolResult::success(vec![Content::text(
                            detail.to_string(),
                        )]));
                    }
                }
                Ok(Err(RecvError::Closed)) | Err(_) => {
                    return Err(McpError::internal_error(
                        "timeout waiting for matching request".to_string(),
                        None,
                    ));
                }
            }
        }
    }

    /// Flush the ring buffer. Does not disconnect live RN clients.
    #[tool(
        description = "Flush the in-memory request ring buffer. Does not disconnect live RN clients."
    )]
    async fn clear_requests(&self) -> Result<CallToolResult, McpError> {
        self.store
            .clear()
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![Content::text(
            json!({"ok": true}).to_string(),
        )]))
    }
}

#[tool_handler]
impl ServerHandler for RustotronMcp {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.protocol_version = ProtocolVersion::LATEST;
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = Implementation::new("rustotron", env!("CARGO_PKG_VERSION"));
        info.instructions = Some(
            "Rustotron captures HTTP requests from React Native apps via the Reactotron \
             wire protocol. Use list_requests to browse recent traffic, get_request for \
             full detail, search_requests to hunt by substring, and wait_for_request to \
             block on a specific in-flight pattern."
                .to_string(),
        );
        info
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::new_bus;
    use crate::protocol::{ApiRequestSide, ApiResponsePayload, ApiResponseSide};
    use crate::store::{self, StoreConfig};
    use serde_json::Value;
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    fn sample_exchange(method: &str, url: &str, status: u16) -> ApiResponsePayload {
        ApiResponsePayload {
            duration: Some(10.0),
            request: ApiRequestSide {
                url: url.to_string(),
                method: Some(method.to_string()),
                data: Value::Null,
                headers: None,
                params: None,
            },
            response: ApiResponseSide {
                status,
                headers: None,
                body: Value::Null,
            },
        }
    }

    /// Builds a running store + mcp service bound to a fresh bus. Returns
    /// the service plus the cancellation token so tests can tear down.
    async fn service_fixture() -> (
        RustotronMcp,
        store::StoreHandle,
        EventBus,
        CancellationToken,
        tokio::task::JoinHandle<()>,
    ) {
        let bus = new_bus(64);
        let token = CancellationToken::new();
        let store_task = store::spawn(StoreConfig::default(), bus.clone(), token.clone());
        let handle = store_task.handle.clone();
        let service = RustotronMcp::new(handle.clone(), bus.clone());
        (service, handle, bus, token, store_task.join)
    }

    fn text_content(result: &CallToolResult) -> String {
        result
            .content
            .first()
            .expect("at least one content block")
            .as_text()
            .expect("text content")
            .text
            .clone()
    }

    #[tokio::test]
    async fn health_reports_version_and_store_stats() {
        let (service, _h, _bus, token, join) = service_fixture().await;
        let result = service.health().await.unwrap();
        let body: Value = serde_json::from_str(&text_content(&result)).unwrap();
        assert_eq!(body.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(
            body.get("version").and_then(|v| v.as_str()),
            Some(env!("CARGO_PKG_VERSION"))
        );
        assert!(body.get("buffered").is_some());
        assert!(body.get("capacity").is_some());
        token.cancel();
        let _ = join.await;
    }

    #[tokio::test]
    async fn list_requests_returns_summaries_respecting_limit() {
        let (service, handle, _bus, token, join) = service_fixture().await;
        for i in 0..5 {
            handle
                .on_response(sample_exchange("GET", &format!("https://x/{i}"), 200), None)
                .await
                .unwrap();
        }
        let params = Parameters(ListRequestsParams {
            limit: Some(3),
            ..Default::default()
        });
        let result = service.list_requests(params).await.unwrap();
        let body: Value = serde_json::from_str(&text_content(&result)).unwrap();
        let arr = body.as_array().unwrap();
        assert_eq!(arr.len(), 3, "limit should cap result count");
        token.cancel();
        let _ = join.await;
    }

    #[tokio::test]
    async fn list_requests_filters_by_method_and_status_class() {
        let (service, handle, _bus, token, join) = service_fixture().await;
        handle
            .on_response(sample_exchange("GET", "https://x/a", 200), None)
            .await
            .unwrap();
        handle
            .on_response(sample_exchange("POST", "https://x/b", 200), None)
            .await
            .unwrap();
        handle
            .on_response(sample_exchange("POST", "https://x/c", 404), None)
            .await
            .unwrap();

        let params = Parameters(ListRequestsParams {
            method: Some("POST".to_string()),
            status_class: Some("2xx".to_string()),
            ..Default::default()
        });
        let result = service.list_requests(params).await.unwrap();
        let arr: Vec<Value> = serde_json::from_str(&text_content(&result)).unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0].get("method").and_then(|v| v.as_str()), Some("POST"));
        assert_eq!(arr[0].get("status").and_then(|v| v.as_u64()), Some(200));
        assert_eq!(
            arr[0].get("url").and_then(|v| v.as_str()),
            Some("https://x/b")
        );
        token.cancel();
        let _ = join.await;
    }

    #[tokio::test]
    async fn get_request_returns_full_detail_by_id() {
        let (service, handle, _bus, token, join) = service_fixture().await;
        let id = handle
            .on_response(sample_exchange("GET", "https://x/a", 200), None)
            .await
            .unwrap();
        let params = Parameters(GetRequestParams {
            id: id.to_string(),
            include_secrets: false,
        });
        let result = service.get_request(params).await.unwrap();
        let body: Value = serde_json::from_str(&text_content(&result)).unwrap();
        assert_eq!(
            body.get("id").and_then(|v| v.as_str()),
            Some(id.to_string().as_str())
        );
        assert_eq!(
            body.get("request")
                .and_then(|r| r.get("url"))
                .and_then(|v| v.as_str()),
            Some("https://x/a")
        );
        token.cancel();
        let _ = join.await;
    }

    #[tokio::test]
    async fn get_request_rejects_unknown_id() {
        let (service, _h, _bus, token, join) = service_fixture().await;
        let params = Parameters(GetRequestParams {
            id: uuid::Uuid::new_v4().to_string(),
            include_secrets: false,
        });
        assert!(service.get_request(params).await.is_err());
        token.cancel();
        let _ = join.await;
    }

    #[tokio::test]
    async fn wait_for_request_resolves_on_matching_live_event() {
        let (service, handle, _bus, token, join) = service_fixture().await;

        // Fire the matching request slightly after the wait starts.
        let producer_handle = handle.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            producer_handle
                .on_response(
                    sample_exchange("POST", "https://api.test/auth/login", 200),
                    None,
                )
                .await
                .unwrap();
        });

        let params = Parameters(WaitForRequestParams {
            url_pattern: "auth/login".to_string(),
            method: Some("POST".to_string()),
            timeout_ms: Some(2_000),
            include_secrets: false,
        });
        let start = tokio::time::Instant::now();
        let result = service.wait_for_request(params).await.unwrap();
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(1),
            "wait_for_request resolved late: {elapsed:?}"
        );
        let body: Value = serde_json::from_str(&text_content(&result)).unwrap();
        assert_eq!(
            body.get("request")
                .and_then(|r| r.get("url"))
                .and_then(|v| v.as_str()),
            Some("https://api.test/auth/login")
        );

        token.cancel();
        let _ = join.await;
    }

    #[tokio::test]
    async fn wait_for_request_returns_existing_buffered_match_immediately() {
        let (service, handle, _bus, token, join) = service_fixture().await;
        handle
            .on_response(
                sample_exchange("GET", "https://api.test/feed?page=1", 200),
                None,
            )
            .await
            .unwrap();

        let params = Parameters(WaitForRequestParams {
            url_pattern: "feed".to_string(),
            method: None,
            timeout_ms: Some(5_000),
            include_secrets: false,
        });
        let start = tokio::time::Instant::now();
        let result = service.wait_for_request(params).await.unwrap();
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(100),
            "existing match should resolve instantly, took {elapsed:?}"
        );
        let body: Value = serde_json::from_str(&text_content(&result)).unwrap();
        assert_eq!(
            body.get("request")
                .and_then(|r| r.get("url"))
                .and_then(|v| v.as_str()),
            Some("https://api.test/feed?page=1")
        );

        token.cancel();
        let _ = join.await;
    }

    #[tokio::test]
    async fn wait_for_request_times_out_when_no_match_arrives() {
        let (service, _h, _bus, token, join) = service_fixture().await;
        let params = Parameters(WaitForRequestParams {
            url_pattern: "never-happens".to_string(),
            method: None,
            timeout_ms: Some(50),
            include_secrets: false,
        });
        let result = service.wait_for_request(params).await;
        assert!(result.is_err(), "expected timeout error");
        token.cancel();
        let _ = join.await;
    }

    #[tokio::test]
    async fn clear_requests_empties_the_store() {
        let (service, handle, _bus, token, join) = service_fixture().await;
        handle
            .on_response(sample_exchange("GET", "https://x", 200), None)
            .await
            .unwrap();
        assert_eq!(handle.stats().await.unwrap().0, 1);
        let _ = service.clear_requests().await.unwrap();
        assert_eq!(handle.stats().await.unwrap().0, 0);
        token.cancel();
        let _ = join.await;
    }

    #[tokio::test]
    async fn search_requests_finds_url_and_body_substrings() {
        let (service, handle, _bus, token, join) = service_fixture().await;
        let mut exchange = sample_exchange("POST", "https://api.test/submit", 200);
        exchange.response.body = Value::String("matching-cookie-string".to_string());
        handle.on_response(exchange, None).await.unwrap();
        handle
            .on_response(sample_exchange("GET", "https://x/other", 200), None)
            .await
            .unwrap();

        let result = service
            .search_requests(Parameters(SearchRequestsParams {
                query: "cookie".to_string(),
                limit: None,
            }))
            .await
            .unwrap();
        let arr: Vec<Value> = serde_json::from_str(&text_content(&result)).unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(
            arr[0].get("url").and_then(|v| v.as_str()),
            Some("https://api.test/submit")
        );

        token.cancel();
        let _ = join.await;
    }
}
