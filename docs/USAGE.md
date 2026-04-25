# rustotron — usage guide

Practical reference for running `rustotron` today (pre-alpha, built from
source). The eventual end-user guide lives in `docs/setup.md` and
assumes Homebrew / `cargo install` — both come online with the Phase 4
release work. Until then, this is the doc.

---

## 1. Build

```bash
# From the repo root:
cargo build --release        # optimized binary — what you'll actually use
cargo build                  # debug build — faster compile, slower runtime

ls target/release/rustotron  # or target/debug/rustotron
```

Everything below uses `cargo run --release --` as the invocation pattern,
but `./target/release/rustotron` works identically once it's built.

The release profile turns on LTO + `codegen-units = 1` + strip; the
resulting binary is ≈ 5 MB on macOS arm64.

---

## 2. The five ways to run it

### 2a. Default mode — live TUI

```bash
cargo run --release
```

Binds the WebSocket server on `ws://127.0.0.1:9090` and opens the
three-pane TUI. Waits for a Reactotron client to connect (see §5 for the
RN side).

Empty state says `waiting for an RN app to connect…`. Press `q` or
`Ctrl+C` to quit; `SIGINT` exits the process with code **130** so scripts
can distinguish a user interrupt from a normal exit.

### 2b. Mock mode — TUI without a server

```bash
cargo run --release -- --mock
```

Skips the WS server and pre-loads 20 fixture rows covering every
HTTP method (`GET/POST/PUT/DELETE/PATCH`) and every status class
(`2xx/3xx/4xx/5xx`). Useful for demos, screenshots, and verifying the
TUI without an RN app running.

### 2c. Tail mode — line-oriented stdout

```bash
cargo run --release -- tail
# 12:04:31 GET    200    42ms /api/users
# 12:04:32 POST   201   340ms /api/transactions

cargo run --release -- tail --json | jq 'select(.status >= 500)'
# {"id":"…","method":"POST","url":"https://api.x/thing","status":503, …}
```

Binds the WS server, writes one request per line to stdout. Pipeable.
Sensitive headers are **always redacted** in tail output (tail output
is the most screenshot/paste-prone surface). `SIGINT` triggers a clean
exit with code 130.

Flags:

| Flag | Default | Effect |
|---|---|---|
| `--json` | off | Emit ndjson (`{id, method, url, status, duration_ms, received_at_ms}`). Color is forced off in json mode. |
| `--color auto|always|never` | `auto` | Applies to column output. `auto` honours `NO_COLOR` + isatty. |
| `--port`, `--host` | 9090, 127.0.0.1 | Override the WS listen address. |
| `-v`/`-vv`/`-vvv` | WARN | Bump tracing verbosity (goes to stderr). |

### 2d. MCP mode — JSON-RPC over stdio, for AI agents

```bash
cargo run --release -- mcp
```

Speaks MCP over stdin/stdout. Tools exposed:

| Tool | Purpose |
|---|---|
| `health` | Version + ring-buffer stats |
| `list_requests(limit?, method?, statusClass?, urlContains?)` | Compact summaries, filters compose |
| `get_request(id, includeSecrets?)` | Full detail for one row |
| `search_requests(query, limit?)` | Substring search across URL **and** bodies (structured JSON is compact-serialised and searched) |
| `wait_for_request(urlPattern, method?, timeoutMs?, includeSecrets?)` | Long-poll for a matching in-flight request |
| `clear_requests()` | Flush the ring buffer |

Critical: stdout carries **only** JSON-RPC frames. Diagnostic logs go to
stderr. Do not mix — it corrupts the protocol.

Smoke test that MCP is wired up correctly:

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"smoketest","version":"0"}}}' \
  | cargo run --release -- mcp
```

You should see exactly one JSON-RPC response on stdout and zero bytes on
stderr (at default verbosity).

See §6 for wiring MCP into Claude Code / Cursor.

### 2e. Config subcommand — inspect the merged config

```bash
cargo run --release -- config path
# /Users/you/Library/Application Support/dev.rustotron.rustotron/config.toml

cargo run --release -- config show
# # rustotron effective configuration
# # source precedence: CLI > env > file > defaults
# port = 9090
# host = "127.0.0.1"
# capacity = 500
# sensitive-headers = ["authorization", "cookie", "set-cookie", "x-api-key", "proxy-authorization"]
# extra-sensitive-headers = []
# ping-interval-ms = 30000
```

`config show` respects CLI flags + env vars so you can verify what a
specific invocation would actually use:

```bash
RUSTOTRON_PORT=9092 cargo run --release -- --host 0.0.0.0 config show
```

---

## 3. TUI controls

Every action works with either mouse **or** keyboard. You never need to
mix (e.g. "click to select, then press Enter").

### 3a. Keyboard

| Keys | Action |
|---|---|
| `j` / `↓` | Select next request |
| `k` / `↑` | Select previous request |
| `PgDn` / `Shift+J` | Scroll detail pane down |
| `PgUp` / `Shift+K` | Scroll detail pane up |
| `Tab` / `l` / `→` | Focus next pane |
| `Shift+Tab` / `h` / `←` | Focus previous pane |
| `q` / `Ctrl+C` / `Esc` | Quit (Esc cancels input modes first) |
| `/` | Open URL-substring filter input |
| `1` – `5` | Toggle method chips (GET / POST / PUT / DELETE / PATCH) |
| `Alt+1` – `Alt+4` | Toggle status-class chips (2xx / 3xx / 4xx / 5xx) |
| `p` | Pause / resume capture (backend-driven — stops store inserts) |
| `c` | Clear ring buffer (opens confirmation modal) |
| `y` / `n` | Confirm / cancel clear modal |
| `z` | Toggle collapse of the detail section closest to the scroll cursor |
| `F` / `Shift+F` | Open / close the detail full-view modal (for > 1 MB bodies) |
| `y` | Copy selected request as cURL (sensitive headers → `<redacted>`) |
| `Y` / `Shift+y` | Copy selected request as **raw** cURL (real header values) |
| `b` / `B` | Copy the pretty-printed body of the selected request to the clipboard (response body, falls back to request body) |
| `M` / `Shift+m` | Toggle mouse capture — turn off to drag-select & copy text with your terminal's native selection. On macOS Terminal/iTerm hold `⌥` (Option) while dragging for column-block selection that ignores the empty pane. Press `M` again to re-enable click/scroll. |

### 3b. Mouse

| Action | Interaction |
|---|---|
| Select a row | Click the row |
| Focus a pane | Click anywhere in it |
| Scroll the request list | Wheel over the list pane |
| Scroll the detail body | Wheel over the detail pane |
| Toggle a method / status chip | Click the chip |
| Open filter input | Click the `[/url]` chip |
| Pause / resume | Click the `[p]` chip |
| Clear | Click the `[c clear]` chip, then `[y — yes]` in the modal |
| Copy cURL | Click `[copy cURL]` in the detail pane header |

### 3c. What's in the status bar

```
 rustotron  listening on ws://127.0.0.1:9090  —  clients: 1  —  rows: 42
 q/Ctrl+C: quit · j/k or ↑/↓: nav · tab: pane · / url · 1-5 methods · …
```

When `--mock` is set, the first line says "mock data — N rows — not
connected to any server" instead. When pause is active, the `[p]` chip
flips to `[paused]` and new events stop being captured (but the store
keeps its buffered rows — they don't get evicted by traffic that would
have arrived while paused).

---

## 4. Configuration

Precedence (highest first): **CLI flags → `RUSTOTRON_*` env vars → config
file → built-in defaults**.

### 4a. Config file

Location:

- macOS: `~/Library/Application Support/dev.rustotron.rustotron/config.toml`
- Linux: `~/.config/rustotron/config.toml`

Not created automatically. Missing file is fine (you get defaults). Run
`cargo run --release -- config path` to see the resolved path on your
machine.

Full schema, with defaults:

```toml
# WS bind address + port.
port = 9090
host = "127.0.0.1"

# Ring-buffer capacity (must be ≥ 1). Rows beyond this evict FIFO.
capacity = 500

# WebSocket server-side ping cadence (ms, must be ≥ 1). Older Reactotron
# clients don't pong — we don't fail the session for missing pongs.
ping-interval-ms = 30000

# REPLACE the default sensitive-header list. If you set this, re-include
# the defaults you still want masked. Most users should use
# `extra-sensitive-headers` instead.
#
# sensitive-headers = ["authorization", "cookie", "set-cookie", "x-api-key", "proxy-authorization"]

# APPEND to the defaults (case-insensitive dedup). This is the
# recommended way to add a custom auth header like X-Company-Token.
extra-sensitive-headers = []
```

### 4b. Env vars

Every top-level field maps to `RUSTOTRON_<UPPERCASE_KEBAB_TO_UNDERSCORES>`:

| Env var | Field |
|---|---|
| `RUSTOTRON_PORT` | `port` |
| `RUSTOTRON_HOST` | `host` |
| `RUSTOTRON_CAPACITY` | `capacity` |
| `RUSTOTRON_PING_INTERVAL_MS` | `ping-interval-ms` |
| `RUSTOTRON_SENSITIVE_HEADERS` | `sensitive-headers` (TOML array — ok for scripts) |
| `RUSTOTRON_EXTRA_SENSITIVE_HEADERS` | `extra-sensitive-headers` |

`RUST_LOG` also works: it takes precedence over `-v` for tracing filters
(e.g. `RUST_LOG=rustotron=trace` for everything, `RUST_LOG=rustotron::server=debug`
for one module).

### 4c. CLI flags

Global (every subcommand):

| Flag | Effect |
|---|---|
| `--port <u16>` | WS port |
| `--host <addr>` | WS bind address |
| `-v` / `-vv` / `-vvv` | Verbosity |
| `--color auto|always|never` | Stdout color policy (tail mode) |
| `--mock` | Default subcommand only — use fixture data |

Run `cargo run --release -- --help` (or `tail --help`, `mcp --help`,
`config --help`) for the full list.

---

## 5. Wiring up a React Native app

Two things in your RN project:

### 5a. Install the Reactotron client

```bash
npm install reactotron-react-native
```

### 5b. Add `src/ReactotronConfig.ts`

```ts
import Reactotron, { networking } from 'reactotron-react-native';

Reactotron
  .configure({
    name: 'my-app',
    host: '127.0.0.1',  // or your Mac's LAN IP on a physical device
    port: 9090,
  })
  .use(networking())
  .connect();

export default Reactotron;
```

Import it **first** in your entry point:

```ts
import './src/ReactotronConfig';  // must be the first import
import { registerRootComponent } from 'expo';
// ...
```

A fully working Expo example lives at `examples/rn-app/`.

### 5c. LAN / physical-device mode

If the RN app runs on a real phone over Wi-Fi:

```bash
cargo run --release -- --host 0.0.0.0
```

In the RN config, set `host:` to the Mac's LAN IP
(`ipconfig getifaddr en0`).

---

## 6. Using MCP from AI agents

### Claude Code

Add to `~/.claude/mcp.json` (or wherever your Claude Code config lives):

```json
{
  "mcpServers": {
    "rustotron": {
      "command": "/absolute/path/to/rustotron",
      "args": ["mcp"]
    }
  }
}
```

Use the release binary absolute path. Claude Code manages the subprocess
and pipes stdio in/out.

### Cursor

Same shape, in Cursor's MCP settings UI.

### Smoke test

```bash
# Binary should respond to an initialize handshake:
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}' \
  | ./target/release/rustotron mcp 2>/dev/null

# Should print one line starting with `{"jsonrpc":"2.0","id":1,…`.
```

### Example agent workflow

```
Agent: wait_for_request(urlPattern="auth/login", method="POST", timeoutMs=15000)
→ blocks until the user tries to log in
→ returns the full exchange (redacted by default)

Agent: get_request(id="<from above>", includeSecrets=true)
→ returns the raw Authorization header so it can diagnose a bearer-token bug
```

---

## 7. Troubleshooting

### "Port 9090 is in use"

Rustotron prints a friendly hint with the exact `lsof` command to find
the offender:

```
failed to bind on 127.0.0.1:9090: Address already in use

hint: another process is listening on that port. Try:
  rustotron --port 9091

Or find the culprit with:
  lsof -iTCP -sTCP:LISTEN -n -P | grep 9090
```

The usual culprit is **upstream Reactotron running on the same port** —
rustotron uses `9090` by default so default-configured RN apps connect
with zero changes, which means rustotron and Reactotron can't both own
the port at once. Run one of them with `--port 9091` to coexist.

### "Rows appear but bodies say `~~~ skipped ~~~`"

That's the Reactotron networking plugin's default behaviour for
`image/*` content types. Expected.

### "Authorization shows as `***` — I need to see the real value"

- **TUI:** press `Y` (capital y) to copy cURL with raw header values.
- **MCP:** call `get_request(id, includeSecrets=true)` or
  `wait_for_request(..., includeSecrets=true)`.
- **Tail:** no raw mode by design — tail output is the most
  screenshot-prone surface.

### "Clicking inside tmux doesn't work"

Add to `~/.tmux.conf`:

```
set -g mouse on
```

and restart the tmux pane (`tmux kill-server` or reload config). Modern
tmux versions emit SGR-encoded mouse events that crossterm understands.

### "The TUI panicked and broke my terminal"

It shouldn't — the panic hook restores alt-screen / cursor / mouse
capture before printing the backtrace. If you end up with a garbled
terminal anyway, `reset` or `tput reset` will fix it.

### "tail prints nothing when piped to jq"

Stdout is line-buffered for TTYs and fully-buffered for pipes.
`--json` + `jq --unbuffered` keeps jq flushing per line:

```bash
cargo run --release -- tail --json | jq --unbuffered 'select(.status >= 500)'
```

---

## 8. Developing on rustotron

### Running tests

```bash
just test          # everything
just lint          # cargo fmt --check + cargo clippy -D warnings
just ci            # both of the above plus a release-mode test pass

cargo test --lib                         # fast, no network
cargo test --test integration_server     # WS server integration (binds ephemeral ports)
cargo test --test integration_chaos      # garbage-input survival test
cargo test --test integration_tui_snapshots  # ratatui TestBackend snapshots
cargo test --test integration_protocol   # fixture replay
```

### Running benches

```bash
cargo bench --bench bus_throughput -- --quick
# bus_fanout/3_subscribers: ~1.2M events/sec on M-series
```

### Updating TUI snapshots

After intentional UI changes:

```bash
INSTA_UPDATE=always cargo test --test integration_tui_snapshots
# Review the diffs in tests/snapshots/ and commit.
```

### Useful `just` recipes

```bash
just build         # debug build
just build-release # release build
just run -- --mock # cargo run passthrough
just watch         # rebuild+test on change (needs cargo-watch)
just fix           # cargo fmt + clippy --fix
```

---

## 9. Known limitations (pre-alpha)

- Not installable via Homebrew / `cargo install` yet — Phase 4.
- No `docs/mcp.md` as a standalone MCP reference — this doc covers the
  core story; a richer version lands with the docs site.
- UTC time display throughout (`HH:MM:SS` formatted from UNIX-epoch
  seconds). No local-timezone conversion, by design — avoids pulling
  `chrono`.
- cURL clipboard path uses `arboard`. On headless CI / SSH without
  `DISPLAY`, clipboard access fails gracefully and the cURL command is
  printed to stderr so you can copy it manually.
- List-pane mouse hit regions anchor by absolute row index rather than
  ratatui's `ListState::offset()` (which is private). Clicks on a
  scrolled list can occasionally mis-target by the scroll amount. Rare
  in practice at default pane sizes.
