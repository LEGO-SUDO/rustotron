# Reactotron wire-protocol fixtures

These `.ndjson` files are the canonical inputs for codec tests
(`tests/protocol/*` once TASK-100 lands) and integration replay tests.

## Format

- Newline-delimited JSON: **one WebSocket text frame per line**.
- Frames are presented in the order they would appear on the wire (client and
  server interleaved).
- Conventionally, lines whose `type` is `setClientId` are server → client; all
  others are client → server. The fixtures contain no other server-originated
  frames in v1.
- No leading/trailing whitespace inside a frame; trailing newline at EOF.

## Files

| File | What it covers |
|---|---|
| `handshake-ok.ndjson` | Minimum viable handshake: a fresh client (no `clientId`) sends `client.intro`; server responds with `setClientId`. |
| `api-request-response-pair.ndjson` | Returning client (carries `clientId`) followed by one full `api.response` for a `POST /api/login` returning `200`. |
| `api-response-orphaned.ndjson` | An `api.response` arriving without a preceding `client.intro`. Demonstrates the codec decoding payloads independently of handshake state. |
| `api-request-pending.ndjson` | A hypothetical `api.request` event (which the official RN networking plugin does **not** emit — see `docs/protocol.md` §5.4 and §6.3). Used only to verify forward-compat: the codec must route this to `Message::Unknown` without erroring. |
| `mixed-with-unknown-types.ndjson` | Realistic session: handshake → `log` → `api.response` (200) → `state.action.complete` → `display` → `benchmark.report` → `customCommand.register` → `asyncStorage.mutation` → an invented `plugin.unknown.future` type → `api.response` (404). Proves that one bad/unknown frame in the middle of a stream does not stop subsequent good frames from decoding. |

## Provenance

These fixtures were **synthesised by hand** from study of the Reactotron source
at commit
[`9dcedf2c`](https://github.com/infinitered/reactotron/tree/9dcedf2c3342d2e8038579293944cdb34d0aa123)
(see `docs/protocol.md` "Version & compat notes"). They were **not** captured
from a live device. Specifically:

- The envelope shape (`type`, `payload`, `important`, `date`, `deltaTime`)
  matches what `reactotron-core-client/src/reactotron-core-client.ts:send()`
  produces.
- `client.intro` field set is a representative subset of what
  `reactotron-react-native/src/reactotron-react-native.ts:DEFAULTS.client`
  emits; we omit a few platform-only fields (`forceTouch`, `interfaceIdiom`,
  `uiMode`, `serial`) to keep the fixtures readable.
- `api.response` payloads match
  `reactotron-core-contract/src/apiResponse.ts:ApiResponsePayload`. The
  `data: "~~~ undefined ~~~"` token in some fixtures reflects the
  client-side falsy-value mangling documented in `docs/protocol.md` §3.2.
- The IDs (`clientId`, request IDs in the orphan/pending fixtures) are made
  up; they are valid GUID-shaped strings or small integers.

## When you need more

If TASK-100 or a downstream task needs a fixture this directory does not
provide:

1. **First** check `docs/protocol.md` to confirm the shape you need is
   real (vs. a phantom from PRD/BUILD_PLAN phrasing — `api.request` is the
   canonical example: the PRD mentions it, but no current Reactotron
   networking plugin emits it).
2. **Then** synthesise a new `.ndjson` here following the formatting rules
   above and add it to the table in this README.
3. **As a last resort**, capture from a live device: run
   `cargo run -- --tap-output <file.ndjson>` (proposed flag, not yet
   implemented as of TASK-003) against an RN app instrumented with
   `reactotron-react-native`. Sanitise any real auth tokens before
   committing.

## Stability

Lines in these files are intentionally stable. If a fixture needs to change,
update both the file and any snapshot tests that pin against it (`insta`
review will flag the diff).
