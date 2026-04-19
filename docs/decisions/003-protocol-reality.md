# ADR-003: Protocol reality vs. PRD framing

**Status:** Accepted
**Date:** 2026-04-19
**Supersedes:** —
**Related:** `docs/protocol.md`, PRD FR-2 / FR-3, BUILD_PLAN TASK-100 / TASK-102
**Triggered by:** TASK-003 source study of `infinitered/reactotron` @
`9dcedf2c3342d2e8038579293944cdb34d0aa123`

## Context

The PRD and BUILD_PLAN were drafted before we studied Reactotron's source. A
few assumptions in them are wrong against the actual wire protocol. This ADR
records the deltas so TASK-100 (codec) and TASK-102 (store) don't inherit
broken framing.

The spec documents (PRD.md, BUILD_PLAN.md) are **not** being rewritten — they
remain as the high-level product intent. This ADR is the ground truth for
implementation.

## Findings from source study

### F-1. No `api.request` frame on the wire

`reactotron-react-native/src/plugins/networking.ts` registers two XHR hooks
(`onSend`, `onResponse`). The `onSend` hook **does not send a WS frame** — it
only caches the request locally and starts a timer. `onResponse` emits a
single `api.response` that carries both the request and response halves.

**Consequence:** the codec has **one** API variant — `Message::ApiResponse` —
not the two (`ApiRequest` + `ApiResponse`) that BUILD_PLAN TASK-100 lists.

### F-2. No `requestId` on the wire

Reactotron's protocol has no per-request id. The networking plugin's internal
counter is process-local and never leaves the client. The server attaches a
monotonic `messageId` on ingest, but that's server-internal and is not part
of the protocol.

**Consequence:** rustotron synthesises its own `RequestId = Uuid::new_v4()`
inside the store actor (TASK-102), not at the codec layer. The codec stays
correlation-free.

### F-3. No "pending" or "orphaned" state from the wire

PRD FR-3 and BUILD_PLAN TASK-102 AC list three correlation outcomes
(`req-then-res`, `res-without-req`, `req-never-resolved`). With F-1/F-2 those
collapse into one observable outcome: every `api.response` is a complete
transaction at the moment it arrives.

**Consequence:**

- `State::Pending` is unobservable in v1. Keep the variant in the store type
  as forward-compat (see F-5) but never produce it from the codec path.
- `State::Orphaned` only exists as a forward-compat fixture for third-party
  plugins that might split events. No current plugin produces it.
- **Every RN request that completes shows up as `State::Complete` with a
  duration.** Requests that never complete (app killed, network timeout
  without an error response) never arrive at all — that's a Reactotron
  limitation we document to users, not a rustotron bug.

### F-4. Falsy-value sentinels — leave literal

`reactotron-core-client/src/serialize.ts` replaces `undefined` / `null` /
`false` / `0` / `""` with string sentinels like `"~~~ undefined ~~~"` before
JSON-stringifying. The upstream server's `repair-serialization.ts` undoes
this on ingest.

**Decision:** rustotron does **not** undo the mapping at v1. The fields we
surface (`status`, `url`, `method`, `headers`, `duration`) are not affected
in practice — they're non-falsy primitives or full objects. For `body` /
`data`, if a sentinel arrives we render it literally — that's a faithful
representation of "RN sent an empty value". Revisit if user reports show
sentinels leaking confusingly into the TUI.

Rationale: `repair()` logic is ~60 lines, recursive, and touches deeply
nested JSON. Pulling it in when it doesn't fix a real problem is
speculative complexity.

### F-5. `api.response` payload shape — confirmed

```rust
// Conceptual — exact struct layout belongs to TASK-100.
pub struct ApiResponsePayload {
    pub duration: Option<f64>,                 // nullable per source
    pub request: ApiRequestSide,
    pub response: ApiResponseSide,
}
pub struct ApiRequestSide {
    pub url: String,
    pub method: Option<String>,                // nullable per source
    pub data: serde_json::Value,               // string | object | sentinel
    pub headers: Option<HashMap<String, String>>,
    pub params: Option<HashMap<String, String>>,
}
pub struct ApiResponseSide {
    pub status: u16,
    pub headers: Option<HashMap<String, String>>,
    pub body: serde_json::Value,               // string | object | sentinel
}
```

### F-6. `client.intro` payload — open-ended extras

Required: `name: String`. Optional: `clientId`, `environment`, plus an
open-ended `client.*` block that the RN DEFAULTS populate
(`platform`, `platformVersion`, `reactNativeVersion`, `screenWidth`, etc.).
The field set grows with each RN release.

**Decision:** model as `name: String` plus a single
`extra: serde_json::Map<String, serde_json::Value>` for all other fields.
The TUI / MCP extract specific keys on demand rather than the codec enforcing
a schema that will drift.

### F-7. `setClientId` server→client — slim envelope

The only server-originated message. Envelope is
`{ "type": "setClientId", "payload": "<guid-string>" }` with **no** `date`
or `deltaTime`. Sent only when the incoming `client.intro` had no
`payload.clientId`.

**Decision:** encode hand-built for this variant — don't reuse the full
envelope serializer with Option fields, because the RN client's dispatcher
only checks `type` and `payload` and we want to mirror the upstream wire
byte-for-byte.

### F-8. Reconnect + duplicate-client behaviour

When a second WS connection carries a `clientId` that already has a live
connection, the upstream server schedules `socket.close()` on the **older**
connection 500 ms later
(`reactotron-core-server.ts:259`).

**Decision:** rustotron mirrors this — closes the older session so the user
sees one row per device after a reload. TASK-103 implements the 500 ms timer
in `server::session`.

### F-9. 30 s server-side ping keepalive

The upstream server pings all clients every 30 s. The RN client does not ping.

**Decision:** rustotron emits server-side pings at the same cadence but does
**not** require pongs — older clients may ignore them. Dead-connection
detection happens via the WS read side closing, not via a pong timeout.
TASK-103 implements this.

## How the PRD / BUILD_PLAN acceptance criteria still apply

The user-facing behaviour the PRD promises is unaffected. We adapt the
implementation:

| PRD / plan item | Still true? | How we meet it |
|---|---|---|
| AC "request followed by matching response → single row with `Complete` state" | Yes | The single `api.response` frame creates one row with `Complete` state. |
| AC "response without prior request → single row with `Orphaned` state" | In letter | Never observable from the official plugin. Store actor supports `State::Orphaned` as a forward-compat path (for 3rd-party plugins that split events); fixture replay exercises it. |
| AC "N+1 insert evicts oldest" | Yes | Store ring buffer, unchanged. |
| FR-2 "Unknown event types decode to an `Unknown` variant" | Yes | Codec has `Message::Unknown(serde_json::Value)`. |
| FR-3 "Handle out-of-order, duplicate, and orphan cases without crashing" | Yes | Out-of-order = store by `date`. Duplicates = accepted, two rows. Orphans = see above. |

## Consequences for downstream tasks

### TASK-100 (codec)

- `Message` has **four** variants: `ClientIntro`, `SetClientId`, `ApiResponse`,
  `Unknown`. No `ApiRequest`.
- `setClientId` encode is hand-built (no date/deltaTime).
- `ClientIntroPayload` uses `name: String` + `extra: Map`.
- `ApiResponsePayload` uses `duration: Option<f64>`, `method: Option<String>`,
  nullable header maps.
- Never returns `Err` for a known `type` whose payload fails to parse — log
  `warn`, fall through to `Unknown`. (Matches NFR-9.)

### TASK-102 (store actor)

- Every `api.response` creates one row in `State::Complete`. No pending rows
  from RN traffic.
- Keep `State::{ Pending, Orphaned }` variants on the `Request` struct —
  they're free to carry and pay for themselves the day a third-party plugin
  splits events.
- `RequestId` generated here (not in codec).
- Correlation logic can be empty in v1; add a TODO for "future split-event
  plugins" with a pointer back to this ADR.

### TASK-103 (WS server)

- Implements the 500 ms duplicate-clientId takeover (F-8).
- Implements 30 s server-side pings without pong timeout (F-9).
- Responds with `setClientId` only when the incoming `client.intro` lacks one.

## Open questions

None blocking. One revisit point: if user reports show falsy-value sentinels
leaking into the TUI in a confusing way, port the upstream `repair()` logic
into `src/protocol/repair.rs`. Not needed for v1.
