# Reactotron wire protocol — reference for the rustotron codec

**Status:** Reference document
**Date:** 2026-04-19
**Audience:** TASK-100 (codec implementer) and anyone debugging a live RN ↔
rustotron session.
**Source studied:** `infinitered/reactotron` @
[`9dcedf2c3342d2e8038579293944cdb34d0aa123`](https://github.com/infinitered/reactotron/tree/9dcedf2c3342d2e8038579293944cdb34d0aa123)
(master HEAD on 2026-04-19). See "Version & compat notes" at the bottom for
package versions and where things might drift.

---

## TL;DR

- One WebSocket connection per RN client. Default port `9090` upstream;
  rustotron defaults to `9090` so both can run at once.
- Text frames, one JSON object per frame. No binary frames. No
  newline-delimited batching inside a frame.
- Client speaks first with `client.intro`. Server replies with `setClientId`
  **only when the client did not provide one**. Otherwise zero server-to-client
  traffic is required — the connection is "live" the moment the WS upgrade
  succeeds.
- Every message uses the same envelope: `{ type, payload, important?, date,
  deltaTime }`. The server adds `messageId`, `connectionId`, `clientId` on
  ingestion (these are server-internal; they are NOT echoed back to clients).
- For our v1 subset we care about three message types: `client.intro`,
  `setClientId`, and `api.response`. Everything else decodes to
  `Message::Unknown` and is silently dropped (FR-2).
- `api.response` is **one** combined message that arrives after the HTTP
  exchange completes. Reactotron does NOT emit a separate "request started"
  event over the wire. There is therefore no `requestId` on the wire — see
  "Correlation semantics" below for how rustotron synthesises one.

---

## 1. Transport

### 1.1 WebSocket

| Aspect | Value |
|---|---|
| Protocol | `ws://` (default). `wss://` supported by upstream server when configured with PFX or cert/key. |
| Default port | `9090` (Reactotron). **rustotron uses `9090`** so we coexist (PRD FR-1). |
| Frame type | Text only. The client serialises every message via `JSON.stringify` (`reactotron-core-client/src/serialize.ts`); server parses with `JSON.parse` (`reactotron-core-server/src/reactotron-core-server.ts:217`). |
| Subprotocol | None. The client opens a plain `new WebSocket("ws://host:port")` (`reactotron-react-native.ts:DEFAULTS.createSocket`). No `Sec-WebSocket-Protocol` header is sent. |
| Origin | None. RN's WebSocket implementation does not set an Origin we can rely on. Do not require it. |
| Path | `/`. Any path the client connects to is accepted; `ws` (the upstream `ws` library) does not route by path. |
| Per-message compression | Off (upstream's `WebSocketServer({ port })` accepts the `ws` defaults; permessage-deflate is negotiated only if the client requests it; the RN `WebSocket` does not). Implementations should accept either. |

### 1.2 Keepalive

The upstream server runs a 30-second `setInterval` that pings every connected
client (`reactotron-core-server.ts:174`). The RN client never pings. Pings are
a server-to-client liveness check; rustotron should send ping frames at the
same cadence but **must not fail** on missing pongs from older clients.

### 1.3 Framing rules

- Exactly one JSON object per frame.
- No newline delimiters inside a frame; `JSON.stringify` is called on the whole
  envelope at once.
- Frames are independent. There is no length prefix, no envelope-of-envelopes,
  no streaming chunks.
- Frame text is UTF-8 (the WebSocket spec requires it for text frames).

### 1.4 Upgrade handshake (HTTP)

Standard RFC 6455 upgrade. Headers sent by the RN client are whatever its
underlying `WebSocket` (Hermes / JSC native impl) provides; we should not
inspect them. `tokio-tungstenite::accept_async` handles the upgrade end-to-end
without any custom logic.

---

## 2. Connection lifecycle

```
RN client                                rustotron server
    │                                          │
    │  TCP SYN / WS upgrade                    │
    │ ───────────────────────────────────────► │
    │                                          │ accept; emit ClientConnected(connId)
    │  ◄───────────────────────────── 101 OK ─ │
    │                                          │
    │  text frame: client.intro {…}            │
    │ ───────────────────────────────────────► │ parse; if payload.clientId is missing,
    │                                          │ generate GUID and reply:
    │  ◄────────────────── { type:"setClientId", payload:"<guid>" }
    │                                          │ promote PartialConnection → Connection;
    │                                          │ emit connectionEstablished
    │                                          │
    │  text frame: api.response {…}            │
    │ ───────────────────────────────────────► │ store + emit ResponseReceived
    │  text frame: log {…}                     │
    │ ───────────────────────────────────────► │ decode → Unknown; drop
    │  …                                       │
    │                                          │
    │  WS close                                │
    │ ──────────────────► / ◄───────────────── │ emit ClientDisconnected(connId)
```

### 2.1 Who speaks first

The **client speaks first**, but only after its app-level `onOpen` fires.
Source: `reactotron-core-client.ts:onOpen()` calls `this.send("client.intro",
…)` from inside the `WebSocket.onopen` callback. There is no server-initiated
greeting; if the server tries to send anything before the intro, the client
ignores it (it has not registered handlers yet).

### 2.2 Acks

There are **no protocol-level acks**. The TCP/WS layer is the only delivery
guarantee. The single piece of server-to-client traffic during handshake is
the conditional `setClientId` frame.

### 2.3 Timing expectations

- The client buffers any messages it tries to send before `onOpen` in
  `sendQueue` and flushes them after `client.intro` (`reactotron-core-client.ts:onOpen`).
- The client's reconnect behaviour is **not** part of the protocol — it is
  delegated to the underlying `WebSocket` and the RN error overlay. rustotron
  must accept that a fresh `client.intro` may arrive on a new connection at
  any time, possibly with the **same `clientId`** the client used previously.
- Upstream server quirk: when a second connection arrives carrying a
  `clientId` that already has a live connection, the server schedules
  `socket.close()` on the **older** connection 500 ms later
  (`reactotron-core-server.ts:259`). Rustotron should mirror this so the user
  sees only one row per device after a reload.

### 2.4 Disconnect

Either side may close. The server emits `disconnect` only for connections
that completed the intro (`reactotron-core-server.ts:201`). Half-handshaked
connections (TCP up but no `client.intro` received) are removed from
`partialConnections` silently — rustotron should match this.

---

## 3. The envelope

Every frame on the wire — both directions — is a JSON object with this shape:

```ts
// Source: reactotron-core-client.ts:send()
//         reactotron-core-server.ts:onMessage handler
interface WireMessage<P = unknown> {
  type: string;        // discriminant; see §4 for the catalog
  payload?: P;         // type-specific; some messages omit it entirely
  important?: boolean; // client-set; defaults to false; "highlight in UI"
  date: string;        // ISO-8601, e.g. "2026-04-19T12:34:56.789Z"
  deltaTime: number;   // ms since the client's previous outbound message; 0 for the first
}
```

Notes:

- `type` is the **only** field rustotron should match on for routing. Treat
  unknown values as `Message::Unknown`.
- `payload` may be `null`, `undefined`, an object, an array, a string, or a
  boolean. Some message types (`clear`, `devtools.open`, `devtools.reload`)
  legitimately have no payload.
- `important` is informational; we ignore it in v1.
- `date` is generated client-side, so clock skew is real. Use server-side
  receipt time for ordering in our store; surface `date` only as the displayed
  timestamp.
- `deltaTime` is also client-side; it can be `0` legitimately (first message,
  or system clock went backwards — see `reactotron-core-client.ts:330` which
  clamps to `0`).
- The server **adds three more fields** before fanning out to its in-process
  listeners: `messageId` (monotonic int per server), `connectionId` (int),
  `clientId` (string). These never travel over the wire to other clients, so
  the rustotron decoder must NOT expect them on incoming frames. We will
  generate our own equivalents inside the store actor.

### 3.1 Server → client envelope (the one case)

The only message rustotron sends back is `setClientId`. Its envelope is
**slimmer**:

```ts
// Source: reactotron-core-server.ts:241
{ type: "setClientId", payload: "<guid-string>" }
```

No `date`, no `deltaTime`. The RN client only inspects `type` and `payload`
(`reactotron-core-client.ts:onMessage → if (command.type === "setClientId")`),
so omitting the timing fields is safe. We mirror that exactly.

### 3.2 Falsy-value mangling — beware

`reactotron-core-client/src/serialize.ts` replaces certain JS values with
sentinel strings before stringification, because the original author wanted to
distinguish `undefined` from "missing key":

| JS value | On the wire |
|---|---|
| `undefined` | `"~~~ undefined ~~~"` |
| `null` | `"~~~ null ~~~"` |
| `false` | `"~~~ false ~~~"` |
| `0` / `-0` | `"~~~ zero ~~~"` |
| `""` | `"~~~ empty string ~~~"` |
| `Infinity` | `"~~~ Infinity ~~~"` |
| `-Infinity` | `"~~~ -Infinity ~~~"` |
| Circular ref | `"~~~ Circular Reference ~~~"` |
| anonymous fn | `"~~~ anonymous function ~~~"` |
| named fn `foo` | `"~~~ foo() ~~~"` |
| `BigInt(n)` | string of `n.toString()` |
| iterator (non-Array) | spread into a JSON array |

The upstream server's `repair()` undoes this mapping (lower-case match) on
ingest before fanning out (`reactotron-core-server/src/repair-serialization.ts`).

**rustotron decision (open question for TASK-100):** for v1, do NOT undo the
mapping. The fields we surface (`status`, `url`, `method`, `headers`,
`duration`) are not affected by the falsy mangling in practice, because they
are either non-falsy primitives or full objects. If a `body` arrives as
`"~~~ empty string ~~~"`, render it literally — that's a faithful
representation of "the server sent zero bytes". Revisit if user reports show
the sentinels leaking into the TUI in a confusing way.

---

## 4. Message-type catalog

The complete list lives in
`reactotron-core-contract/src/command.ts → CommandType`. Reproduced here so the
codec author can write the discriminant without going back to the source.

| `type` | Direction | In v1 scope? | Payload type |
|---|---|---|---|
| `client.intro` | client → server | **yes** | `ClientIntroPayload` |
| `setClientId` | server → client | **yes** | `string` (the GUID) |
| `api.response` | client → server | **yes** | `ApiResponsePayload` |
| `log` | client → server | drop → `Unknown` | `LogPayload` |
| `display` | client → server | drop → `Unknown` | `DisplayPayload` |
| `image` | client → server | drop → `Unknown` | `ImagePayload` |
| `benchmark.report` | client → server | drop → `Unknown` | `BenchmarkReportPayload` |
| `clear` | server → client | drop → `Unknown` | `undefined` |
| `custom` | bidirectional | drop → `Unknown` | string \| `{ command, args }` |
| `customCommand.register` | client → server | drop → `Unknown` | `CustomCommandRegisterPayload` |
| `customCommand.unregister` | client → server | drop → `Unknown` | `CustomCommandUnregisterPayload` |
| `asyncStorage.mutation` | client → server | drop → `Unknown` | `AsyncStorageMutationPayload` |
| `saga.task.complete` | client → server | drop → `Unknown` | `SagaTaskCompletePayload` |
| `state.action.complete` | client → server | drop → `Unknown` | `StateActionCompletePayload` |
| `state.action.dispatch` | server → client | drop → `Unknown` | `StateActionDispatchPayload` |
| `state.backup.request` | server → client | drop → `Unknown` | `StateBackupRequestPayload` |
| `state.backup.response` | client → server | drop → `Unknown` | `StateBackupResponsePayload` |
| `state.restore.request` | server → client | drop → `Unknown` | `StateRestoreRequestPayload` |
| `state.keys.request` | server → client | drop → `Unknown` | `StateKeysRequestPayload` |
| `state.keys.response` | client → server | drop → `Unknown` | `StateKeysResponsePayload` |
| `state.values.request` | server → client | drop → `Unknown` | `StateValuesRequestPayload` |
| `state.values.response` | client → server | drop → `Unknown` | `StateValuesResponsePayload` |
| `state.values.change` | client → server | drop → `Unknown` | `StateValuesChangePayload` |
| `state.values.subscribe` | server → client | drop → `Unknown` | `StateValuesSubscribePayload` |
| `repl.ls.response` | client → server | drop → `Unknown` | `ReplLsResponsePayload` |
| `repl.execute.response` | client → server | drop → `Unknown` | `ReplExecuteResponsePayload` |
| `devtools.open` | server → client | drop → `Unknown` | `undefined` |
| `devtools.reload` | server → client | drop → `Unknown` | `undefined` |
| `editor.open` | server → client | drop → `Unknown` | `EditorOpenPayload` |
| `storybook` | server → client | drop → `Unknown` | `boolean` |
| `overlay` | server → client | drop → `Unknown` | `boolean` |

The codec's `Message` enum should have a final variant
`Unknown(serde_json::Value)` so unknown `type` strings (which Reactotron does
ship in plugins outside this list, e.g. `reactotron-mst` adds its own) round-
trip without erroring (FR-2 forward compat).

---

## 5. The three messages we care about

### 5.1 `client.intro` (client → server)

**Source of truth:**
`reactotron-core-contract/src/clientIntro.ts` and the `client` block in
`reactotron-react-native.ts:DEFAULTS`.

**Schema:**

```ts
interface ClientIntroPayload {
  // From the contract package
  name: string;                   // required; the app's display name
  clientId?: string;              // optional; if absent, server assigns one
  environment?: string;           // "development" | "production" | etc.
  reactotronVersion?: string;     // historical; rarely populated

  // Added by the server on receipt — NOT sent by client
  address?: string;

  // De-facto fields the RN client always sends (from `client: {…}` in DEFAULTS):
  reactotronLibraryName?: string;        // "reactotron-react-native"
  reactotronLibraryVersion?: string;     // "5.1.18" or template literal placeholder
  platform?: "ios" | "android" | "web";
  platformVersion?: string | number;
  osRelease?: string;
  model?: string;
  serverHost?: string;
  forceTouch?: boolean;
  interfaceIdiom?: string;
  systemName?: string;
  uiMode?: string;
  serial?: string;
  reactNativeVersion?: string;
  screenWidth?: number;
  screenHeight?: number;
  screenScale?: number;
}
```

**Required vs optional:** Per the contract, `name` is the only formally
required field. In practice, RN clients always send `name`, `clientId` (after
the first connect, persisted in AsyncStorage), and the long `client.*` block.
The codec should require `name` and treat the rest as optional. Missing
`clientId` triggers our `setClientId` reply.

**Wire example (full envelope):**

```json
{
  "type": "client.intro",
  "payload": {
    "name": "MyAwesomeApp",
    "environment": "development",
    "clientId": "f3a4b1c2-1234-5678-9abc-def012345678",
    "reactotronLibraryName": "reactotron-react-native",
    "reactotronLibraryVersion": "5.1.18",
    "platform": "ios",
    "platformVersion": "17.4",
    "osRelease": "23E224",
    "model": "iPhone15,2",
    "systemName": "iOS",
    "screenWidth": 390,
    "screenHeight": 844,
    "screenScale": 3,
    "reactNativeVersion": "0.74.1"
  },
  "important": false,
  "date": "2026-04-19T12:34:56.000Z",
  "deltaTime": 0
}
```

### 5.2 `setClientId` (server → client)

**Schema:** `{ type: "setClientId", payload: "<guid>" }` — no other envelope
fields. The GUID is generated server-side via the eight-`s4()` pattern in
`reactotron-core-server.ts:createGuid()`. Format is roughly UUID-shaped but
**not** a real UUID — it's eight 4-hex-digit chunks with `-` separators.
rustotron will use a real `uuid::Uuid::new_v4()` instead; the client only
echoes the string back, so format does not matter.

**Sent only when** the incoming `client.intro` had no `payload.clientId`.

### 5.3 `api.response` (client → server)

**Source of truth:**
`reactotron-core-contract/src/apiResponse.ts` and
`reactotron-react-native/src/plugins/networking.ts`.

**Schema:**

```ts
interface ApiResponsePayload {
  duration: number;          // ms, float; from a stopwatch started at XHR.send()
  request: {
    url: string;             // full URL, query string included
    method: string | null;   // "GET" / "POST" / "PUT" / "PATCH" / "DELETE" / "HEAD" / etc.
    data: any;               // body the app passed to XHR.send(); often a string of JSON
    headers: Record<string, string> | null;
    params: Record<string, string> | null; // parsed from the query string by the plugin
  };
  response: {
    status: number;          // HTTP status code, e.g. 200, 404, 500
    headers: Record<string, string> | null;
    body: any;               // see "body shape" below
  };
}
```

**Body shape:** `networking.ts` tries `JSON.parse(responseBodyText)` first; on
failure it falls back to the raw `response` string. So `body` may be:

- A parsed object/array (the common case for JSON APIs).
- A string (non-JSON responses, or images/binary if not skipped).
- The literal string `"~~~ skipped ~~~"` — the plugin substitutes this when
  the response body is empty.
- Skipped for `Content-Type: image/*` by default
  (`networking.ts:DEFAULT_CONTENT_TYPES_RX`); in that case `body` ends up as
  `"~~~ skipped ~~~"`.

**`duration` semantics:** populated from `reactotron.startTimer()`, which
under RN uses `global.nativePerformanceNow` (sub-millisecond) when available,
falling back to `Date.now()` deltas. Treat it as a `f64` number of
milliseconds. Can theoretically be `null` if the timer was never started (the
plugin guards: `stopTimer ? stopTimer() : null`); in practice it is always
present for matched req/res pairs, but the codec should accept `null`.

**Wire example (full envelope):**

```json
{
  "type": "api.response",
  "payload": {
    "duration": 142.7,
    "request": {
      "url": "https://api.example.com/login?retry=1",
      "method": "POST",
      "data": "{\"email\":\"jane@example.com\",\"password\":\"hunter2\"}",
      "headers": {
        "Content-Type": "application/json",
        "Authorization": "Bearer eyJhbGciOiJIUzI1NiJ9...",
        "User-Agent": "MyAwesomeApp/1.0 (iOS 17.4)"
      },
      "params": { "retry": "1" }
    },
    "response": {
      "status": 200,
      "headers": {
        "Content-Type": "application/json",
        "Set-Cookie": "session=abc123; HttpOnly"
      },
      "body": {
        "user": { "id": 42, "email": "jane@example.com" },
        "token": "eyJhbGciOiJIUzI1NiJ9..."
      }
    }
  },
  "important": false,
  "date": "2026-04-19T12:34:57.143Z",
  "deltaTime": 1143
}
```

### 5.4 `api.request` — does it exist?

**No.** This is the single most important finding for the codec author. The RN
networking plugin (`reactotron-react-native/src/plugins/networking.ts`) does
**not** emit a separate event when a request begins. It registers two
XHRInterceptor callbacks:

- `onSend` — caches the request locally, starts the timer. **No WS message
  sent.**
- `onResponse` — assembles the matched `request` + `response` and emits a
  single `api.response` event.

Therefore `api.request` is a Reactotron type **we will never see on the wire**
from the official networking plugin. Some third-party plugins may emit it; if
so, our `Unknown` arm catches them. We do not need an `api.request` variant in
the v1 codec, despite the BUILD_PLAN's task name implying we do.

> **Naming hygiene for the implementer:** the BUILD_PLAN spec uses
> "`api.request`" colloquially. Inside `src/protocol/api_events.rs` we should
> have a single `ApiResponse` (or `HttpExchange`) struct that contains both
> the `request` and `response` halves, mirroring `ApiResponsePayload`. There
> is no `ApiRequest` struct to write.

---

## 6. Correlation semantics

### 6.1 `requestId` does not exist on the wire

Reactotron has no concept of a `requestId` in its protocol. The networking
plugin's per-request counter (`reactotronCounter`, starting at 1000) lives
**inside the RN process** and is only used to match `onSend` to `onResponse`
locally. By the time the WS frame arrives, the request and response are
already glued together in one `api.response` payload.

Server-side, every received message gets a monotonic `messageId`
(`reactotron-core-server.ts:54, 222`), but this is internal — it is **not**
echoed back, and it is not stable across server restarts. We will mirror this
internally in the store actor for our own correlation, but it is not a
protocol concept.

### 6.2 What rustotron does instead

The store actor (TASK-102) generates a `RequestId = Uuid::new_v4()` per
incoming `api.response` and uses it as the row key in the ring buffer. From
the codec's perspective, **no correlation logic is needed** — every
`api.response` is a complete, self-contained transaction.

### 6.3 Edge cases (and how they look on the wire)

| Scenario | What we see | Decoder behaviour |
|---|---|---|
| Normal request → response | One `api.response` frame after the response completes | Emit one `RequestStarted` + `ResponseReceived` event, both with the same generated `RequestId` |
| Response with no preceding request | Same as above — there's no separate request frame to be missing | Indistinguishable from normal; nothing to flag |
| Request with no response (timeout, app killed) | **No frame at all** — the plugin only sends after `onResponse` fires | We will never know about it; this is a Reactotron limitation, not a rustotron bug |
| Out-of-order responses (slow request finishes after fast one started later) | Two `api.response` frames in completion order | Just store both; UI sorts by `date` |
| Duplicate frame (same request repeated) | Two distinct `api.response` frames with identical content | Store both; we do not deduplicate |
| Client reconnect mid-request | The in-flight request's response was queued in `sendQueue`; on reconnect, the client re-sends `client.intro` then flushes the queue | We get the `api.response` slightly delayed but normally |

The "request-with-no-response" and "response-with-no-request" cases mentioned
in PRD FR-3 and BUILD_PLAN TASK-003 therefore manifest very differently from
how the spec phrasing implies:

- "Response without prior request": **never happens** in standard usage,
  because a single combined frame carries both. The fixture
  `api-response-orphaned.ndjson` simulates this only as a forward-compat test
  for hypothetical third-party plugins that split the events.
- "Pending request": **never observable over the wire** — pending means "no
  frame received yet". The fixture `api-request-pending.ndjson` documents the
  hypothetical split-event shape and is not produced by any current plugin.

If a future Reactotron plugin (or an alternate networking plugin) does start
emitting separate `api.request` + `api.response` frames, the implementer
should:

1. Add an `ApiRequest` codec variant.
2. Carry the local counter as a `requestId` field (the upstream
   `reactotronCounter` is the natural source).
3. Match by `requestId` in the store actor; use a short-lived map.

For v1, none of that exists.

---

## 7. Events we silently drop in v1

All of these decode to `Message::Unknown(serde_json::Value)` and are
discarded by the store actor:

- **Logging:** `log` (debug/warn/error levels)
- **State management:** `state.action.complete`, `state.action.dispatch`,
  `state.backup.request`, `state.backup.response`, `state.restore.request`,
  `state.keys.request`, `state.keys.response`, `state.values.request`,
  `state.values.response`, `state.values.change`, `state.values.subscribe`
- **Display helpers:** `display`, `image`
- **Benchmarks:** `benchmark.report`
- **Sagas:** `saga.task.complete`
- **Custom:** `custom`, `customCommand.register`, `customCommand.unregister`
- **AsyncStorage:** `asyncStorage.mutation`
- **REPL:** `repl.ls.response`, `repl.execute.response`
- **Editor / devtools / overlay:** `editor.open`, `devtools.open`,
  `devtools.reload`, `storybook`, `overlay`, `clear`

Forward-compat policy: the `Unknown` arm captures the entire envelope
(including unknown `type` strings we have never seen), logs at `trace`, and is
otherwise a no-op. The decoder must never return an `Err` for an unknown
`type` — only for malformed JSON or a missing `type` field.

---

## 8. Validation we owe at decode time

| Field | Validation | Action on failure |
|---|---|---|
| Frame is valid UTF-8 text | by tungstenite | drop frame, log at `warn` |
| Frame is valid JSON | `serde_json::from_str` | drop frame, log at `warn` |
| Top-level is an object | first deserialize step | drop frame, log at `warn` |
| `type` is a string | required for routing | drop frame, log at `warn` |
| `payload` shape matches the variant | per-variant `Deserialize` | for `client.intro` and `api.response`: drop and log at `warn`; for everything else: fall through to `Unknown` |
| `date` parses as ISO-8601 | `chrono::DateTime::parse_from_rfc3339` | accept the message but use server receipt time (do not drop) |
| `deltaTime` is a non-negative number | `serde_json::Value::as_f64` then check | accept; clamp negatives to 0 (matches client's own behaviour) |

NFR-9 says malformed messages must not crash; the table above is the contract.

---

## 9. Worked sample exchanges

### 9.1 Minimum viable handshake

```
client →  {"type":"client.intro","payload":{"name":"DemoApp","environment":"development"},"important":false,"date":"2026-04-19T12:00:00.000Z","deltaTime":0}
server →  {"type":"setClientId","payload":"7f3c1e2a-9b4d-4c5f-8a6e-1d2c3b4a5f60"}
```

### 9.2 Returning client (already has a clientId)

```
client →  {"type":"client.intro","payload":{"name":"DemoApp","clientId":"7f3c1e2a-9b4d-4c5f-8a6e-1d2c3b4a5f60","environment":"development"},"important":false,"date":"2026-04-19T12:01:00.000Z","deltaTime":0}
( server sends nothing )
```

### 9.3 A request/response pair

See `tests/fixtures/reactotron-traces/api-request-response-pair.ndjson` for
the canonical version.

---

## 10. Implementation checklist for the rustotron codec

1. [ ] `Message` enum with variants: `ClientIntro(ClientIntroPayload)`,
       `SetClientId(String)`, `ApiResponse(ApiResponsePayload)`,
       `Unknown(serde_json::Value)`. Use `#[serde(tag = "type", content = "payload", rename_all = "lowercase")]` won't quite work because the tag values use dot notation — use `#[serde(tag = "type")]` with explicit `#[serde(rename = "...")]` per variant, or a custom `Deserialize` impl.
2. [ ] `Envelope<P>` struct with `type` (skipped on deserialize because it's
       the discriminant), `payload`, `important: bool` (default false), `date:
       Option<DateTime<Utc>>` (None on parse failure), `deltaTime: f64`
       (default 0.0).
3. [ ] `ClientIntroPayload` with required `name: String` and a single
       `extra: serde_json::Map<String, Value>` for everything else (the field
       set is too unstable to model). The TUI will pluck specific keys
       (`platform`, `platformVersion`, etc.) on demand.
4. [ ] `ApiResponsePayload` with `duration: Option<f64>` (yes, optional —
       see §5.3), `request: ApiRequestSide`, `response: ApiResponseSide`,
       both with sub-fields per §5.3. `body` and `data` typed as
       `serde_json::Value` to absorb either parsed JSON or raw strings.
5. [ ] `encode(&Message) -> Result<String>` uses `serde_json::to_string`.
       For `setClientId`, hand-build the slim envelope (no `date` /
       `deltaTime`).
6. [ ] `decode(&str) -> Result<Message>` returns `Message::Unknown(value)`
       for any `type` not in the v1 list. **Never** returns `Err` for a known
       `type` whose payload fails to parse — log at `warn` and fall through
       to `Unknown` so a single bad frame doesn't poison the variant.
7. [ ] Round-trip property test: every fixture in
       `tests/fixtures/reactotron-traces/` decodes without error, and known
       variants re-encode to byte-identical JSON modulo key order.
8. [ ] No `panic!`, no `unwrap()` on `serde_json` results in this module.

---

## 11. Version & compat notes

Pinned source: `infinitered/reactotron` @ commit
`9dcedf2c3342d2e8038579293944cdb34d0aa123` (master HEAD on 2026-04-19).
Package versions at this commit:

| Package | Version |
|---|---|
| `reactotron-core-server` | 3.2.1 |
| `reactotron-core-client` | 2.9.9 |
| `reactotron-react-native` | 5.1.18 |
| `reactotron-core-contract` | 0.3.2 |

### Things that look stable

- The envelope `{ type, payload, important, date, deltaTime }` has been the
  same shape since the project's nx-rewrite era. Safe to depend on.
- `client.intro` → `setClientId` handshake has been the same for years.
- `api.response` payload shape has been the same since the networking plugin
  was extracted from `reactotron-react-native` ~v4.

### Things that might drift — flag at implementation time

- The `client.*` block sent inside `client.intro` grows over time. New RN
  versions add fields (e.g. `reactNativeVersion` is conditional on RN ≥ 0.62;
  `serial` only on Android; `forceTouch` only on iOS). Treating it as an
  open-ended `Map<String, Value>` (per checklist item 3) is the right call.
- The body-mangling sentinel format (`~~~ undefined ~~~` etc.) has been
  questioned in upstream issues; if upstream removes it, our "render literal"
  policy still works.
- `reactotron-core-client/src/plugins/api-response.ts` shows that the
  **client-side** `apiResponse` API takes `(request, response, duration)` as
  three positional args. If a community plugin accidentally swaps these, we
  see weird `duration` values; defensive parsing per §5.3 covers it.
- `reactotronVersion` field in `ClientIntroPayload` (per the contract) is
  documented but we never see it sent — RN's DEFAULTS use
  `reactotronLibraryVersion` instead. Treat both as optional.

### Things to re-check on the next protocol-source bump

- New `CommandType` entries (means new "drop to Unknown" types — verify the
  `Unknown` arm still catches them).
- Any move from text frames to binary frames (currently no such PR open).
- Any addition of a real `requestId` field to `ApiResponsePayload` (would
  upstream gain HTTP/2 multiplexing visibility? unlikely for v1).

### Source files studied

For posterity and so a reviewer can spot-check our claims:

- `lib/reactotron-core-server/src/reactotron-core-server.ts` (12 KB)
- `lib/reactotron-core-server/src/repair-serialization.ts`
- `lib/reactotron-core-server/src/validation.ts`
- `lib/reactotron-core-client/src/reactotron-core-client.ts` (16 KB)
- `lib/reactotron-core-client/src/client-options.ts`
- `lib/reactotron-core-client/src/serialize.ts`
- `lib/reactotron-core-client/src/stopwatch.ts`
- `lib/reactotron-core-client/src/plugins/api-response.ts`
- `lib/reactotron-react-native/src/reactotron-react-native.ts`
- `lib/reactotron-react-native/src/xhr-interceptor.ts`
- `lib/reactotron-react-native/src/plugins/networking.ts`
- `lib/reactotron-core-contract/src/{clientIntro,apiResponse,command,server-events,log,state,display,image,benchmark,asyncStorage,customCommand}.ts`

All quoted snippets are short fair-use excerpts; the rustotron tree contains
no Reactotron source verbatim.
