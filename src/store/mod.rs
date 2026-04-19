//! In-memory request store, actor-pattern.
//!
//! One tokio task owns the `VecDeque<Request>` ring buffer. Every other
//! surface talks to it through [`StoreHandle`] — a cheap-to-clone
//! `mpsc::Sender<Cmd>` wrapper. Queries reply via `oneshot` channels.
//!
//! See `docs/decisions/002-concurrency-model.md` for the rationale.

pub mod actor;
pub mod config;
pub mod redact;
pub mod request;

use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::bus::{EventBus, RequestId};
use crate::protocol::ApiResponsePayload;

pub use self::config::{DEFAULT_CAPACITY, StoreConfig};
pub use self::redact::{REDACTION_MASK, default_sensitive_headers};
pub use self::request::{Request, SecretsMode, State};

use self::actor::Cmd;

/// Error returned when the store actor has stopped and a handle can no
/// longer serve requests.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The actor task has exited — either cancellation fired, or the
    /// actor panicked (which is not expected in production code).
    #[error("store actor has shut down")]
    Closed,
}

/// Cheap-to-clone handle to the store.
///
/// Every surface (WS session, TUI, MCP tools, tail task) holds one.
/// Dropping the last handle signals the actor to exit.
#[derive(Debug, Clone)]
pub struct StoreHandle {
    tx: mpsc::Sender<Cmd>,
}

/// Bundle returned by [`spawn`] — keep this alive for the actor's lifetime.
/// Drop `handle` to stop accepting new commands; the task's `JoinHandle`
/// is included so callers can `.await` clean shutdown.
#[derive(Debug)]
pub struct StoreTask {
    /// Public handle to hand out to other surfaces.
    pub handle: StoreHandle,
    /// Join handle for the actor task. Awaiting it returns after the
    /// actor has processed its final command.
    pub join: JoinHandle<()>,
}

/// Spawn the store actor on the current runtime.
///
/// `bus` is the domain event channel the store publishes to on insert.
/// `token` is the shared cancellation signal — typically the one rooted
/// in `main` that also covers the WS server and TUI tasks.
#[must_use]
pub fn spawn(config: StoreConfig, bus: EventBus, token: CancellationToken) -> StoreTask {
    // Mpsc channel sized for the highest realistic burst: multiple WS
    // clients each committing a response, plus occasional queries from
    // the TUI. 256 covers this with 2+ orders of magnitude of margin on
    // single-digit ms drain latency.
    let (tx, rx) = mpsc::channel(256);
    let join = tokio::spawn(actor::run(rx, config, bus, token));
    StoreTask {
        handle: StoreHandle { tx },
        join,
    }
}

impl StoreHandle {
    /// Commit a completed `api.response` to the ring buffer. Returns the
    /// generated `RequestId` of the new row.
    ///
    /// # Errors
    ///
    /// [`StoreError::Closed`] if the actor is no longer running.
    pub async fn on_response(
        &self,
        exchange: ApiResponsePayload,
        client_id: Option<crate::bus::ClientId>,
    ) -> Result<RequestId, StoreError> {
        let request = Request::complete(exchange, client_id);
        let id = request.id;
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::Insert {
                request: Box::new(request),
                reply,
            })
            .await
            .map_err(|_| StoreError::Closed)?;
        rx.await.map_err(|_| StoreError::Closed)?;
        Ok(id)
    }

    /// Return every row currently buffered, oldest → newest.
    ///
    /// # Errors
    ///
    /// [`StoreError::Closed`] if the actor is no longer running.
    pub async fn all(&self, mode: SecretsMode) -> Result<Vec<Request>, StoreError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::All { mode, reply })
            .await
            .map_err(|_| StoreError::Closed)?;
        rx.await.map_err(|_| StoreError::Closed)
    }

    /// Return a specific row by id.
    ///
    /// # Errors
    ///
    /// [`StoreError::Closed`] if the actor is no longer running.
    pub async fn get(
        &self,
        id: RequestId,
        mode: SecretsMode,
    ) -> Result<Option<Request>, StoreError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::Get { id, mode, reply })
            .await
            .map_err(|_| StoreError::Closed)?;
        rx.await.map_err(|_| StoreError::Closed)
    }

    /// Drop every row. Does not publish bus events.
    ///
    /// # Errors
    ///
    /// [`StoreError::Closed`] if the actor is no longer running.
    pub async fn clear(&self) -> Result<(), StoreError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::Clear { reply })
            .await
            .map_err(|_| StoreError::Closed)?;
        rx.await.map_err(|_| StoreError::Closed)
    }

    /// Pause or resume capture. While paused, subsequent `on_response`
    /// calls are acknowledged but **not** added to the ring buffer and no
    /// bus event is published — this is the backend-side pause the PRD
    /// (FR-11) requires. Clients already connected stay connected.
    ///
    /// # Errors
    ///
    /// [`StoreError::Closed`] if the actor is no longer running.
    pub async fn set_paused(&self, paused: bool) -> Result<(), StoreError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::SetPaused { paused, reply })
            .await
            .map_err(|_| StoreError::Closed)?;
        rx.await.map_err(|_| StoreError::Closed)
    }

    /// Return `(buffered_rows, capacity)`.
    ///
    /// # Errors
    ///
    /// [`StoreError::Closed`] if the actor is no longer running.
    pub async fn stats(&self) -> Result<(usize, usize), StoreError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::Stats { reply })
            .await
            .map_err(|_| StoreError::Closed)?;
        rx.await.map_err(|_| StoreError::Closed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{Event, new_bus};
    use crate::protocol::{ApiRequestSide, ApiResponsePayload, ApiResponseSide};
    use serde_json::Value;
    use std::collections::HashMap;
    use tokio::sync::broadcast::error::TryRecvError;

    fn make_exchange(url: &str, status: u16, auth: Option<&str>) -> ApiResponsePayload {
        let mut req_headers: HashMap<String, String> = HashMap::new();
        if let Some(a) = auth {
            req_headers.insert("Authorization".to_string(), a.to_string());
        }
        ApiResponsePayload {
            duration: Some(10.0),
            request: ApiRequestSide {
                url: url.to_string(),
                method: Some("GET".to_string()),
                data: Value::Null,
                headers: Some(req_headers),
                params: None,
            },
            response: ApiResponseSide {
                status,
                headers: None,
                body: Value::Null,
            },
        }
    }

    async fn start(config: StoreConfig) -> (StoreTask, EventBus, CancellationToken) {
        let bus = new_bus(64);
        let token = CancellationToken::new();
        let task = spawn(config, bus.clone(), token.clone());
        (task, bus, token)
    }

    #[tokio::test]
    async fn on_response_creates_complete_row_and_publishes_event() {
        let (task, bus, token) = start(StoreConfig::default()).await;
        let mut rx = bus.subscribe();
        let id = task
            .handle
            .on_response(make_exchange("https://x/a", 200, None), None)
            .await
            .unwrap();

        // Buffer holds exactly one row, with our id and Complete state.
        let rows = task.handle.all(SecretsMode::Raw).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, id);
        assert_eq!(rows[0].state, State::Complete);

        // Bus saw exactly the event we expect.
        match rx.try_recv() {
            Ok(Event::ResponseReceived(got)) => assert_eq!(got, id),
            other => panic!("expected ResponseReceived, got {other:?}"),
        }

        token.cancel();
        let _ = task.join.await;
    }

    #[tokio::test]
    async fn ring_eviction_drops_oldest_on_capacity_plus_one() {
        let cfg = StoreConfig::with_capacity(3);
        let (task, _bus, token) = start(cfg).await;

        // Insert 4 into a 3-slot ring; the first should be evicted.
        let mut ids = Vec::new();
        for i in 0..4 {
            let id = task
                .handle
                .on_response(make_exchange(&format!("https://x/{i}"), 200, None), None)
                .await
                .unwrap();
            ids.push(id);
        }

        let (len, cap) = task.handle.stats().await.unwrap();
        assert_eq!(len, 3);
        assert_eq!(cap, 3);

        let rows = task.handle.all(SecretsMode::Raw).await.unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].id, ids[1], "oldest was evicted");
        assert_eq!(rows[2].id, ids[3], "newest is at the tail");

        token.cancel();
        let _ = task.join.await;
    }

    #[tokio::test]
    async fn get_returns_the_row_when_present_or_none_otherwise() {
        let (task, _bus, token) = start(StoreConfig::default()).await;
        let id = task
            .handle
            .on_response(make_exchange("https://x", 200, None), None)
            .await
            .unwrap();

        let found = task.handle.get(id, SecretsMode::Raw).await.unwrap();
        assert!(found.is_some());

        let missing = task
            .handle
            .get(RequestId::new(), SecretsMode::Raw)
            .await
            .unwrap();
        assert!(missing.is_none());

        token.cancel();
        let _ = task.join.await;
    }

    #[tokio::test]
    async fn all_with_redacted_mode_masks_sensitive_headers() {
        let (task, _bus, token) = start(StoreConfig::default()).await;
        task.handle
            .on_response(
                make_exchange("https://x", 200, Some("Bearer real-token")),
                None,
            )
            .await
            .unwrap();

        // Raw view preserves the token.
        let raw = task.handle.all(SecretsMode::Raw).await.unwrap();
        assert_eq!(
            raw[0]
                .exchange
                .request
                .headers
                .as_ref()
                .and_then(|h| h.get("Authorization"))
                .map(String::as_str),
            Some("Bearer real-token")
        );

        // Redacted view masks it.
        let red = task.handle.all(SecretsMode::Redacted).await.unwrap();
        assert_eq!(
            red[0]
                .exchange
                .request
                .headers
                .as_ref()
                .and_then(|h| h.get("Authorization"))
                .map(String::as_str),
            Some(REDACTION_MASK)
        );

        token.cancel();
        let _ = task.join.await;
    }

    #[tokio::test]
    async fn clear_empties_the_buffer() {
        let (task, _bus, token) = start(StoreConfig::default()).await;
        for _ in 0..3 {
            task.handle
                .on_response(make_exchange("https://x", 200, None), None)
                .await
                .unwrap();
        }
        assert_eq!(task.handle.stats().await.unwrap().0, 3);
        task.handle.clear().await.unwrap();
        assert_eq!(task.handle.stats().await.unwrap().0, 0);

        token.cancel();
        let _ = task.join.await;
    }

    #[tokio::test]
    async fn cancellation_stops_the_actor_and_handles_return_closed() {
        let (task, _bus, token) = start(StoreConfig::default()).await;
        token.cancel();
        let _ = task.join.await;

        match task.handle.stats().await {
            Err(StoreError::Closed) => {}
            other => panic!("expected Closed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dropping_all_handles_shuts_down_the_actor_gracefully() {
        let (task, _bus, _token) = start(StoreConfig::default()).await;
        drop(task.handle);
        // Without any remaining handles the mpsc channel closes and the
        // actor exits its run loop.
        let outcome = tokio::time::timeout(std::time::Duration::from_millis(500), task.join).await;
        assert!(
            outcome.is_ok(),
            "actor should exit within 500 ms of the last handle being dropped"
        );
    }

    #[tokio::test]
    async fn set_paused_stops_commits_and_events() {
        let (task, bus, token) = start(StoreConfig::default()).await;
        let mut rx = bus.subscribe();

        task.handle.set_paused(true).await.unwrap();
        // Insert while paused — should be acknowledged but dropped.
        task.handle
            .on_response(make_exchange("https://x/paused", 200, None), None)
            .await
            .unwrap();
        assert_eq!(task.handle.stats().await.unwrap().0, 0);
        assert!(matches!(
            rx.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));

        // Resume and commit.
        task.handle.set_paused(false).await.unwrap();
        task.handle
            .on_response(make_exchange("https://x/live", 200, None), None)
            .await
            .unwrap();
        assert_eq!(task.handle.stats().await.unwrap().0, 1);
        assert!(matches!(rx.try_recv(), Ok(Event::ResponseReceived(_))));

        token.cancel();
        let _ = task.join.await;
    }

    #[tokio::test]
    async fn bus_receives_one_response_received_event_per_insert() {
        let (task, bus, token) = start(StoreConfig::default()).await;
        let mut rx = bus.subscribe();

        for _ in 0..3 {
            task.handle
                .on_response(make_exchange("https://x", 200, None), None)
                .await
                .unwrap();
        }

        let mut count = 0usize;
        loop {
            match rx.try_recv() {
                Ok(Event::ResponseReceived(_)) => count += 1,
                Ok(other) => panic!("unexpected event: {other:?}"),
                Err(TryRecvError::Empty) => break,
                Err(e) => panic!("unexpected try_recv error: {e}"),
            }
        }
        assert_eq!(count, 3);

        token.cancel();
        let _ = task.join.await;
    }
}
