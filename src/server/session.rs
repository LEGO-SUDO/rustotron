//! Per-connection WebSocket session task.
//!
//! Reads frames, decodes via [`crate::protocol`], forwards `api.response`
//! payloads to the store, and conditionally replies with `setClientId` on
//! the first `client.intro` that lacks one. Emits `ClientConnected` /
//! `ClientDisconnected` bus events bracketing the handshake.

use std::net::SocketAddr;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::bus::{ClientId, Event, EventBus};
use crate::protocol::{self, Message};
use crate::store::StoreHandle;

use super::RegistryCmd;
use super::config::ServerConfig;

/// Context passed to a session — bundles the shared handles so the
/// session entry point stays under clippy's argument-count limit.
pub(super) struct SessionCtx {
    /// Server configuration (ping cadence, etc.).
    pub config: ServerConfig,
    /// Handle to the in-memory store.
    pub store: StoreHandle,
    /// Domain event publisher.
    pub bus: EventBus,
    /// Cancellation token for this specific session.
    pub token: CancellationToken,
    /// Channel back to the accept loop's takeover registry.
    pub registry: tokio::sync::mpsc::Sender<RegistryCmd>,
}

/// Drive one accepted WebSocket connection until close or cancellation.
///
/// Must not panic. Any error on the wire becomes a `warn` log and a clean
/// close — per PRD NFR-9 a single misbehaving client never takes down the
/// server.
pub(super) async fn run_session(ws: WebSocketStream<TcpStream>, peer: SocketAddr, ctx: SessionCtx) {
    let SessionCtx {
        config,
        store,
        bus,
        token,
        registry,
    } = ctx;
    let our_client_id = ClientId::new();
    let (mut sink, mut stream) = ws.split();
    let mut ping_ticker = tokio::time::interval(config.ping_interval);
    // Consume the immediate first tick so we don't ping before we've
    // sent any traffic.
    ping_ticker.tick().await;
    let mut announced = false;

    tracing::debug!(%peer, %our_client_id, "session started");

    loop {
        tokio::select! {
            biased;
            () = token.cancelled() => {
                tracing::debug!(%our_client_id, "session cancelled; closing");
                let _ = sink.send(WsMessage::Close(None)).await;
                break;
            }
            _ = ping_ticker.tick() => {
                // Ping payload is empty; we never consult pong (ADR-003 §F-9).
                if let Err(e) = sink.send(WsMessage::Ping(Vec::new().into())).await {
                    tracing::debug!(error = %e, %our_client_id, "ping send failed; closing");
                    break;
                }
            }
            frame = stream.next() => match frame {
                None => {
                    tracing::debug!(%our_client_id, "peer closed stream");
                    break;
                }
                Some(Err(e)) => {
                    tracing::warn!(error = %e, %our_client_id, "ws read error; closing");
                    break;
                }
                Some(Ok(WsMessage::Text(text))) => {
                    let mut ctx = HandleTextCtx {
                        sink: &mut sink,
                        announced: &mut announced,
                        our_client_id,
                        store: &store,
                        bus: &bus,
                        registry: &registry,
                        token: &token,
                    };
                    handle_text(text.as_str(), &mut ctx).await;
                }
                Some(Ok(WsMessage::Binary(_))) => {
                    tracing::warn!(%our_client_id, "unexpected binary frame; dropping");
                }
                Some(Ok(WsMessage::Close(_))) => {
                    tracing::debug!(%our_client_id, "peer sent close");
                    break;
                }
                Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_))) => {
                    // tungstenite auto-responds to pings; nothing to do.
                }
                Some(Ok(WsMessage::Frame(_))) => {
                    // Low-level raw-frame pass-through; never produced by
                    // accept_async. Ignore defensively.
                }
            }
        }
    }

    if announced {
        // Only publish disconnect if we actually announced connect.
        let _ = bus.send(Event::ClientDisconnected(our_client_id));
    }
    // Always deregister from the takeover registry. The accept loop
    // ignores the Leave if we've already been replaced by a newer
    // session.
    let _ = registry
        .send(RegistryCmd::Leave { our: our_client_id })
        .await;
    tracing::debug!(%our_client_id, "session ended");
}

struct HandleTextCtx<'a> {
    sink: &'a mut futures_util::stream::SplitSink<WebSocketStream<TcpStream>, WsMessage>,
    announced: &'a mut bool,
    our_client_id: ClientId,
    store: &'a StoreHandle,
    bus: &'a EventBus,
    registry: &'a tokio::sync::mpsc::Sender<RegistryCmd>,
    token: &'a CancellationToken,
}

async fn handle_text(text: &str, ctx: &mut HandleTextCtx<'_>) {
    let our_client_id = ctx.our_client_id;
    match protocol::decode(text) {
        Ok(Message::ClientIntro(intro)) => {
            if !*ctx.announced {
                let _ = ctx.bus.send(Event::ClientConnected(our_client_id));
                *ctx.announced = true;
                tracing::info!(
                    %our_client_id,
                    app = %intro.name,
                    has_client_id = intro.client_id.is_some(),
                    "client.intro received"
                );
            }
            // Announce to the registry whenever the client provides a
            // stable `clientId`. If another session was already under
            // that id, the accept loop schedules its takeover after the
            // documented 500 ms grace (H-3 / ADR-003 §F-8).
            if let Some(cid) = intro.client_id.clone() {
                let _ = ctx
                    .registry
                    .send(RegistryCmd::Announce {
                        our: our_client_id,
                        client_id: cid,
                        shutdown: ctx.token.clone(),
                    })
                    .await;
            }
            if intro.client_id.is_none() {
                // Mirror upstream's slim-envelope setClientId reply (ADR-003 §F-7).
                let guid = Uuid::new_v4().to_string();
                match protocol::encode(&Message::SetClientId(guid)) {
                    Ok(frame) => {
                        if let Err(e) = ctx.sink.send(WsMessage::Text(frame.into())).await {
                            tracing::warn!(error = %e, %our_client_id, "failed to send setClientId");
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "failed to encode setClientId (unreachable)");
                    }
                }
            }
        }
        Ok(Message::ApiResponse(payload)) => {
            match ctx.store.on_response(payload, Some(our_client_id)).await {
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "store unavailable; dropping api.response");
                }
            }
        }
        Ok(Message::SetClientId(_)) => {
            // Server→client only. Misbehaving client — ignore.
            tracing::warn!(%our_client_id, "client sent setClientId; ignoring");
        }
        Ok(Message::Unknown(_)) => {
            // Silently dropped per PRD FR-2.
        }
        Err(e) => {
            tracing::warn!(error = %e, "frame decode failed; dropping");
        }
    }
}
