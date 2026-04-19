//! `Request` — the stored representation of one observed HTTP exchange.
//!
//! A thin wrapper over `ApiResponsePayload` with store-side bookkeeping
//! (generated id, server-receipt time, originating client, state tag).
//!
//! `State::Pending` and `State::Orphaned` are reserved for forward-compat
//! with third-party RN plugins that might split `api.request` /
//! `api.response` into separate frames (ADR-003 §F-3). The official plugin
//! never produces them — every row created from the wire path at v1 is
//! `State::Complete`.

use std::time::SystemTime;

use crate::bus::{ClientId, RequestId};
use crate::protocol::ApiResponsePayload;

/// Lifecycle tag for a stored row.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum State {
    /// Forward-compat: "we saw a request frame, still waiting for the
    /// matching response." Not produced by the v1 wire path.
    Pending,
    /// Normal path: request + response both observed.
    Complete,
    /// Forward-compat: "a response arrived with no matching prior
    /// request." Not produced by the v1 wire path.
    Orphaned,
}

/// Controls whether sensitive headers are masked at read time.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SecretsMode {
    /// Sensitive headers show `***`. Default for TUI list and MCP
    /// `get_request` without `include_secrets`.
    Redacted,
    /// Sensitive headers show their raw values. Requires the user to
    /// explicitly opt in (TUI toggle, MCP `include_secrets: true`).
    Raw,
}

/// One stored exchange. **Clone is NOT cheap** — `serde_json::Value` and
/// `HashMap<String, String>` both perform deep copies. Redacted reads
/// clone every row they return, which is fine for 500-row default
/// capacity but can matter if users crank capacity up or send multi-MB
/// bodies. If memory/CPU show up in profiling, the path is `Arc<ApiResponsePayload>`
/// (single clone of the Arc, shared body, redaction on read via Cow) or
/// a summary/detail split where the list view only holds the shape it
/// needs.
#[derive(Debug, Clone)]
pub struct Request {
    /// Store-generated id. Stable for the lifetime of this process.
    pub id: RequestId,
    /// Wall-clock time the store received this exchange.
    pub received_at: SystemTime,
    /// Lifecycle tag — see [`State`].
    pub state: State,
    /// Which connected RN client produced this exchange, if known.
    pub client_id: Option<ClientId>,
    /// The full observed exchange — request + response halves.
    pub exchange: ApiResponsePayload,
}

impl Request {
    /// Construct a `Complete` row from a fresh `api.response` payload.
    ///
    /// This is the only path v1 produces. Pending / Orphaned rows, if ever
    /// needed, must be constructed directly via struct-literal syntax —
    /// we deliberately avoid a constructor so the "not from the wire"
    /// provenance is visible at call sites.
    #[must_use]
    pub fn complete(exchange: ApiResponsePayload, client_id: Option<ClientId>) -> Self {
        Self {
            id: RequestId::new(),
            received_at: SystemTime::now(),
            state: State::Complete,
            client_id,
            exchange,
        }
    }
}
