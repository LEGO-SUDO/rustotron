//! `StoreActor` — owns the `VecDeque<Request>` ring buffer and serves it
//! through an mpsc command channel. Intended to be spawned once in `main`;
//! every other task interacts via [`StoreHandle`](super::StoreHandle).
//!
//! Shutdown is triggered either by the supplied `CancellationToken` or by
//! all `StoreHandle` clones being dropped (which closes the command
//! channel).

use std::collections::VecDeque;

use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::bus::{Event, EventBus};

use super::config::StoreConfig;
use super::redact::apply_secrets_mode;
use super::request::{Request, SecretsMode};

/// Commands the actor understands. `#[non_exhaustive]` because future
/// tasks (filters, search, wait-for) will add variants.
#[derive(Debug)]
#[non_exhaustive]
pub(super) enum Cmd {
    /// Commit a completed exchange to the ring buffer.
    Insert {
        request: Box<Request>,
        reply: oneshot::Sender<()>,
    },
    /// Return every row currently in the buffer, oldest → newest, with
    /// `mode` applied to header values.
    All {
        mode: SecretsMode,
        reply: oneshot::Sender<Vec<Request>>,
    },
    /// Return the row with the given id, if present.
    Get {
        id: crate::bus::RequestId,
        mode: SecretsMode,
        reply: oneshot::Sender<Option<Request>>,
    },
    /// Drop every row. Does not publish bus events.
    Clear { reply: oneshot::Sender<()> },
    /// Report current `(len, capacity)`. Useful in tests and the status bar.
    Stats {
        reply: oneshot::Sender<(usize, usize)>,
    },
    /// Toggle capture. When paused, `Insert` commands are acknowledged but
    /// the row is **not** added to the ring buffer and no bus event is
    /// published. This is the backend-side pause that PRD FR-11 requires
    /// — a TUI-only pause is observably broken (rows keep evicting while
    /// the user thinks capture is frozen).
    SetPaused {
        paused: bool,
        reply: oneshot::Sender<()>,
    },
}

/// Run the actor until either `rx` closes or `token` is cancelled.
pub(super) async fn run(
    mut rx: mpsc::Receiver<Cmd>,
    config: StoreConfig,
    bus: EventBus,
    token: CancellationToken,
) {
    let mut buffer: VecDeque<Request> = VecDeque::with_capacity(config.capacity);
    let mut paused = false;

    loop {
        tokio::select! {
            biased;
            () = token.cancelled() => {
                tracing::debug!("store actor cancelled");
                break;
            }
            msg = rx.recv() => match msg {
                Some(cmd) => handle(cmd, &mut buffer, &mut paused, &config, &bus),
                None => {
                    tracing::debug!("all StoreHandle clones dropped; store actor exiting");
                    break;
                }
            }
        }
    }
}

fn handle(
    cmd: Cmd,
    buffer: &mut VecDeque<Request>,
    paused: &mut bool,
    config: &StoreConfig,
    bus: &EventBus,
) {
    match cmd {
        Cmd::Insert { request, reply } => {
            if *paused {
                // Acknowledge so the WS session stays responsive, but do
                // NOT commit and do NOT publish on the bus. This is the
                // backend-side pause (H-1).
                let _ = reply.send(());
                return;
            }
            let id = request.id;
            if buffer.len() == config.capacity {
                // Ring eviction — drop the oldest to make room.
                buffer.pop_front();
            }
            buffer.push_back(*request);
            // Ignore send errors — if the publisher has no subscribers yet
            // that's fine, the store is still authoritative.
            let _ = bus.send(Event::ResponseReceived(id));
            // Reply last, so the caller sees the event is published.
            let _ = reply.send(());
        }
        Cmd::All { mode, reply } => {
            let rows = buffer
                .iter()
                .map(|r| apply_secrets_mode(r, mode, &config.sensitive_headers))
                .collect();
            let _ = reply.send(rows);
        }
        Cmd::Get { id, mode, reply } => {
            let row = buffer
                .iter()
                .find(|r| r.id == id)
                .map(|r| apply_secrets_mode(r, mode, &config.sensitive_headers));
            let _ = reply.send(row);
        }
        Cmd::Clear { reply } => {
            buffer.clear();
            let _ = reply.send(());
        }
        Cmd::Stats { reply } => {
            let _ = reply.send((buffer.len(), config.capacity));
        }
        Cmd::SetPaused {
            paused: want,
            reply,
        } => {
            *paused = want;
            let _ = reply.send(());
        }
    }
}
