# ADR-002: Concurrency model — actor + broadcast, no shared state

**Status:** Accepted
**Date:** 2026-04-19
**Supersedes:** —
**Related:** ADR-001 (dependency set)

## Context

`rustotron` runs:

- One WebSocket server task, accepting N concurrent RN client connections.
- One per-connection task per client, decoding frames into domain events.
- One store — a ring buffer of requests — that every surface queries.
- One TUI task (when in TUI mode), rendering at 30 fps, consuming events.
- One MCP server task (when in MCP mode), holding per-tool state and
  long-polling for `wait_for_request`.
- One tail task (when in tail mode), streaming completed requests to stdout.

Naive async Rust designs for "multiple tasks observe and mutate shared state"
end up with `Arc<Mutex<T>>` everywhere, which fights the runtime, creates
poisoning bugs under panic, and teaches everyone the wrong lesson. We make
that impossible by construction.

## Decisions

### D-1. Store is a single-owner actor

One tokio task owns the `VecDeque<Request>` ring buffer. Nothing else touches
it. All reads and writes arrive as messages on an `mpsc::Sender<StoreCmd>`.
Queries reply via a `oneshot::Sender` embedded in the command.

```rust
// Sketch — final shape lives in src/store/.
enum StoreCmd {
    OnRequest { req: ApiRequest, reply: oneshot::Sender<()> },
    OnResponse { res: ApiResponse, reply: oneshot::Sender<()> },
    All { reply: oneshot::Sender<Vec<Request>> },
    Get { id: RequestId, reply: oneshot::Sender<Option<Request>> },
    Clear { reply: oneshot::Sender<()> },
}

#[derive(Clone)]
pub struct StoreHandle { tx: mpsc::Sender<StoreCmd> }
```

Consequences:

- `StoreHandle` is cheap to clone (`Arc<mpsc::Sender<_>>` internally). Every
  surface gets its own handle.
- Race conditions at the data-structure level are architecturally impossible —
  one owner, serial access.
- A stuck consumer cannot deadlock the store. The actor returns results via
  `oneshot`; if the caller drops before `await`, the reply vanishes cleanly.

### D-2. `tokio::sync::broadcast` fans events to N subscribers

All surfaces (TUI, MCP, tail) subscribe to one `broadcast::Sender<Event>` to
learn that "a new request arrived" or "a response completed". The store
actor publishes after committing to the buffer; nothing else publishes.

```rust
// src/bus.rs
pub enum Event {
    RequestStarted(RequestId),
    ResponseReceived(RequestId),
    ClientConnected(ClientId),
    ClientDisconnected(ClientId),
}
pub type EventBus = broadcast::Sender<Event>;
```

Consequences:

- Lagging subscribers get `RecvError::Lagged(n)` and catch up by querying the
  store for current state. Publisher is never blocked by slow consumers —
  critical for the TUI when a panel is paused.
- New subscribers join at runtime (e.g. an MCP `wait_for_request` call spins
  up a short-lived subscriber).

Channel capacity: **1024** slots. Sized for the 500k events/sec bench target
(NFR-4) with 3+ subscribers that each drain within ~2 ms. If we see Lagged
errors in hot paths on benchmarks, bump and document.

### D-3. TUI owns its local state; never shares it across tasks

The Ratatui `App` struct is mutated only inside the TUI task. Its fields
(selected index, filter state, scroll offsets, registered `HitRegion`s) live
on the stack of that task's `run()` loop. When the TUI needs data from the
store, it calls `StoreHandle` and awaits.

Consequences:

- No `Arc<Mutex<App>>`. No render glitches from cross-task mutation.
- The TUI's `tokio::select!` loop multiplexes three sources: crossterm event
  stream, broadcast receiver, render tick. State transitions happen in one
  place.

### D-4. No `Arc<Mutex<T>>` in hot paths without an ADR exception

Clippy is configured to disallow `std::sync::Mutex` and `std::sync::RwLock`
at the type and constructor level (`.clippy.toml`). Those block the runtime
and have no place in async code.

`tokio::sync::Mutex` is allowed but must be justified in a PR description —
it's rarely the right tool. If a reviewer sees it without justification, the
PR bounces.

`Arc<RwLock<T>>` in cold-path code (e.g. one-time config loading, lazy cache
of parsed syntect themes) is permitted when:

1. The lock is acquired during startup or at a user-initiated boundary, and
2. The critical section is measured in microseconds, and
3. A comment explains why channels wouldn't fit.

The three current cold-path exceptions we anticipate:

- Syntect theme / syntax cache: parsed once at first use, read-only
  thereafter. Natural fit for `std::sync::OnceLock`, not `RwLock` — preferred.
- Global config snapshot: loaded once in `main`, then read-only — `OnceLock`.
- Redaction key list: populated from config at start, read-only — `OnceLock`.

So in practice, zero `RwLock`. `OnceLock` covers the read-only-after-startup
pattern without taking a runtime lock at all.

### D-5. Graceful shutdown via `CancellationToken`

`tokio_util::sync::CancellationToken` is the single shutdown signal. `main`
holds the parent; every long-lived task is spawned with a child token.
SIGINT/SIGTERM (`tokio::signal`) triggers `token.cancel()`. Every task
selects on `token.cancelled()` alongside its primary work.

Target: ≤ 500 ms from signal to exit (NFR-10). Exit code 130 on SIGINT.

### D-6. No `spawn_blocking` unless the work is actually blocking

Reserved for three cases:

- Syntect's initial syntax-set parse (CPU-heavy, one-shot at startup).
- Clipboard write in cURL export (some backends block on X11 round-trip).
- Nothing else at v1.

Using `spawn_blocking` as a shortcut around an `async` fn that does a quick
synchronous thing wastes a runtime thread and is a code smell. Reviewer
rejects.

## Consequences for reviewers

- Any PR introducing `Arc<Mutex<T>>` outside this ADR's listed exceptions
  must either (a) extend this ADR with a new exception and rationale, or
  (b) redesign around channels.
- `std::sync::Mutex` / `std::sync::RwLock` in new code fails clippy in CI.
- The actor pattern applies beyond the store — if a future feature needs
  shared mutable state, the default answer is "spawn a task that owns it".

## Open questions (non-blocking)

- Do we want a typed `StoreQuery` / `StoreMutation` split (two enums) for
  readability? Deferred to TASK-102 implementation; punt until we see the
  variants settle.
- Broadcast capacity of 1024 is a guess; will be revisited once TASK-101's
  criterion bench is in place.
