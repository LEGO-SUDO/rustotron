//! WebSocket server — accepts Reactotron client connections and routes
//! decoded frames into the store + bus.
//!
//! One `run(…)` future per server. Spawn it from `main` (or a test) and
//! drop `token.cancel()` to stop. Sessions are spawned as detached tasks
//! on the current runtime; the accept loop tracks them in a `JoinSet` so
//! shutdown can bound wait time.
//!
//! See `docs/decisions/003-protocol-reality.md` for the protocol-level
//! decisions this module implements (setClientId reply policy, ping
//! cadence, etc.).

pub mod config;
pub mod session;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::bus::{ClientId, EventBus};
use crate::store::StoreHandle;

pub use self::config::{DEFAULT_PING_INTERVAL, DEFAULT_PORT, ServerConfig};

/// Failure modes for [`run`]. Runtime errors inside a session (bad frames,
/// peer disconnects) are logged and swallowed — they never surface here.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    /// Could not bind the TCP listener. Usually "port in use"; the caller
    /// should surface a friendly message with an alternate-port hint (PRD
    /// TASK-301).
    #[error("failed to bind ws listener on {addr}: {source}")]
    Bind {
        /// The address we attempted to bind.
        addr: SocketAddr,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },
}

/// Bundle returned by [`bind`] — the listener plus the address it actually
/// bound to (important when the caller asked for port 0).
pub struct BoundServer {
    listener: TcpListener,
    addr: SocketAddr,
}

impl BoundServer {
    /// Socket the server is actually listening on. Useful for tests that
    /// bind to port 0 and need to connect back.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }
}

/// Bind the listener synchronously (well, awaits once) so tests can read
/// back the chosen port before spawning the accept loop.
///
/// # Errors
///
/// [`ServerError::Bind`] if the listener can't claim the address.
pub async fn bind(config: &ServerConfig) -> Result<BoundServer, ServerError> {
    let addr = SocketAddr::new(config.host, config.port);
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|source| ServerError::Bind { addr, source })?;
    let actual = listener
        .local_addr()
        .map_err(|source| ServerError::Bind { addr, source })?;
    Ok(BoundServer {
        listener,
        addr: actual,
    })
}

/// How long after a duplicate-`clientId` connection arrives before we
/// close the older session. Matches upstream Reactotron behaviour
/// (ADR-003 §F-8).
const TAKEOVER_GRACE: Duration = Duration::from_millis(500);

/// Commands sessions send to the accept-loop registry. Used to
/// implement ADR-003 §F-8 (duplicate-`clientId` takeover) without
/// anyone holding a lock — a channel to the accept loop is enough.
#[derive(Debug)]
pub(super) enum RegistryCmd {
    /// A session has received a `client.intro` carrying a specific
    /// client-supplied id. The accept loop should schedule a takeover
    /// of any prior session with the same id.
    Announce {
        /// The new session's internal id.
        our: ClientId,
        /// The client-supplied id from `client.intro.clientId`.
        client_id: String,
        /// Cancellation token whose firing will terminate the new
        /// session. Stored so a subsequent duplicate connection can
        /// cancel us.
        shutdown: CancellationToken,
    },
    /// The session is exiting (for any reason). Remove its entry from
    /// the registry so the next reconnect does not take over a dead
    /// session.
    Leave {
        /// The internal id of the departing session.
        our: ClientId,
    },
}

/// Run the accept loop on the current runtime, pre-bound.
///
/// Returns when `token` is cancelled, after all in-flight session tasks
/// have been given up to 500 ms to wind down (PRD NFR-10 budget).
pub async fn serve(
    bound: BoundServer,
    config: ServerConfig,
    store: StoreHandle,
    bus: EventBus,
    token: CancellationToken,
) {
    tracing::info!(addr = %bound.addr, "ws server listening");

    let mut sessions = JoinSet::new();
    // Registry channel. Capacity 64 is plenty — Announce/Leave are
    // infrequent vs. normal traffic.
    let (registry_tx, mut registry_rx) = mpsc::channel::<RegistryCmd>(64);
    let mut by_client_id: HashMap<String, (ClientId, CancellationToken)> = HashMap::new();

    loop {
        tokio::select! {
            biased;
            () = token.cancelled() => {
                tracing::debug!("server cancelled; stopping accept loop");
                break;
            }
            Some(cmd) = registry_rx.recv() => {
                handle_registry_cmd(cmd, &mut by_client_id, &mut sessions);
            }
            accept = bound.listener.accept() => match accept {
                Ok((tcp, peer)) => {
                    let store = store.clone();
                    let bus = bus.clone();
                    let session_token = token.child_token();
                    let registry = registry_tx.clone();
                    let config = config.clone();
                    sessions.spawn(async move {
                        match tokio_tungstenite::accept_async(tcp).await {
                            Ok(ws) => {
                                session::run_session(
                                    ws,
                                    peer,
                                    session::SessionCtx {
                                        config,
                                        store,
                                        bus,
                                        token: session_token,
                                        registry,
                                    },
                                )
                                .await;
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, %peer, "ws upgrade failed");
                            }
                        }
                    });
                }
                Err(e) => {
                    tracing::warn!(error = %e, "tcp accept failed");
                }
            }
        }
    }

    // Give sessions a bounded window to see cancellation and exit cleanly.
    let drain = tokio::time::timeout(Duration::from_millis(500), async {
        while sessions.join_next().await.is_some() {}
    })
    .await;
    if drain.is_err() {
        tracing::warn!("some sessions did not exit within 500ms; aborting");
        sessions.abort_all();
    }
    tracing::info!("ws server stopped");
}

/// Apply one registry command to the map. Returns immediately —
/// scheduling the 500 ms takeover timer happens on a detached task.
fn handle_registry_cmd(
    cmd: RegistryCmd,
    by_client_id: &mut HashMap<String, (ClientId, CancellationToken)>,
    sessions: &mut JoinSet<()>,
) {
    match cmd {
        RegistryCmd::Announce {
            our,
            client_id,
            shutdown,
        } => {
            let prev = by_client_id.insert(client_id.clone(), (our, shutdown));
            if let Some((prev_id, prev_token)) = prev {
                tracing::info!(
                    %client_id,
                    replaced_session = %prev_id,
                    "duplicate clientId — scheduling takeover of older session in {ms} ms",
                    ms = TAKEOVER_GRACE.as_millis(),
                );
                // Schedule the old session's cancel after the documented
                // grace window, mirroring upstream (ADR-003 §F-8).
                sessions.spawn(async move {
                    tokio::time::sleep(TAKEOVER_GRACE).await;
                    prev_token.cancel();
                });
            }
        }
        RegistryCmd::Leave { our } => {
            // Only remove the entry if it still points at *us* — we do
            // not want a late Leave from a replaced session to kick out
            // the new owner.
            by_client_id.retain(|_, (id, _)| *id != our);
        }
    }
}

/// Convenience: `bind` + `serve`. Use this when the caller does not need
/// the bound address (i.e. production; tests usually call `bind` then
/// `serve` so they can read `local_addr` before spawning).
///
/// # Errors
///
/// Propagates [`ServerError::Bind`] from [`bind`].
pub async fn run(
    config: ServerConfig,
    store: StoreHandle,
    bus: EventBus,
    token: CancellationToken,
) -> Result<(), ServerError> {
    let bound = bind(&config).await?;
    serve(bound, config, store, bus, token).await;
    Ok(())
}
