# Setup — from zero to inspecting RN traffic in 10 minutes

This guide walks through every step a first-time user takes: installing
rustotron, wiring the Reactotron client into a React Native app, and
verifying that requests flow end-to-end.

> **TL;DR:**
> ```bash
> cargo install rustotron      # or: brew install LEGO-SUDO/rustotron/rustotron
> cd my-rn-app && npm install reactotron-react-native
> # add ReactotronConfig.ts (see §2)
> rustotron                    # in one terminal
> npx expo start               # in another
> ```

---

## 1. Install rustotron

### macOS (Homebrew — recommended)

```bash
brew install LEGO-SUDO/rustotron/rustotron
```

### Any platform with Rust toolchain

```bash
cargo install rustotron
```

### Binary downloads

Prebuilt tarballs for `x86_64-apple-darwin`, `aarch64-apple-darwin`,
`x86_64-unknown-linux-gnu`, and `aarch64-unknown-linux-gnu-musl` are
published on each GitHub release.

### Verify

```bash
rustotron --version
rustotron config path     # prints where rustotron looks for config.toml
rustotron --help
```

---

## 2. Wire the RN client

Only two things are required:

1. Install the client + networking plugin.
2. Import a config file near the top of your entry point.

### 2a. Install

```bash
# In your RN project (Expo or bare):
npm install reactotron-react-native
```

### 2b. Add `src/ReactotronConfig.ts`

```ts
import Reactotron, { networking } from 'reactotron-react-native';

Reactotron
  .configure({
    name: 'my-awesome-app',
    host: '127.0.0.1',   // or your Mac's LAN IP on a real device
    port: 9090,           // rustotron default — same port as Reactotron, so existing apps work unchanged.
  })
  .use(networking())
  .connect();

export default Reactotron;
```

### 2c. Import it first in `App.tsx` (or `index.ts`)

```ts
import './src/ReactotronConfig';
// ...everything else below
import { registerRootComponent } from 'expo';
import App from './App';
registerRootComponent(App);
```

**Order matters.** The config has to run before any `fetch` / XHR call so
the XHRInterceptor can see every request.

A working reference is in `examples/rn-app/` in the rustotron repo.

---

## 3. Launch

Two terminals, two commands.

```bash
# Terminal 1 — inspector
rustotron
```

You'll see the TUI with an empty request list and
`listening on ws://127.0.0.1:9090 | clients: 0 | rows: 0` in the status
bar.

```bash
# Terminal 2 — your RN app
cd my-rn-app
npx expo start --ios      # or --android, or press `w` for web
```

Within a second of the app launching, the status bar should jump to
`clients: 1` and any fetch/XHR request should appear in the list pane.

---

## 4. Alternate surfaces

### 4a. Pipe requests to stdout

```bash
rustotron tail                      # human-readable columns
rustotron tail --json | jq          # ndjson, pipeable
rustotron tail --json | jq 'select(.status >= 500)'
```

### 4b. Expose to AI agents over MCP

```bash
rustotron mcp                       # speaks MCP over stdio
```

Claude Code / Cursor config snippets live in `docs/mcp.md`.

---

## 5. Common pitfalls

### "Nothing shows up"

1. Both terminals on the same machine? If your RN app runs on a physical
   device over USB / LAN, point `host:` at your Mac's LAN IP
   (`ipconfig getifaddr en0`) and start rustotron with `--host 0.0.0.0`.
2. Inside tmux? Enable mouse capture: `set -g mouse on` in `~/.tmux.conf`.

### "Port 9090 is in use"

Another rustotron / Reactotron / random process already owns it:

```bash
rustotron --port 9092
# then in ReactotronConfig.ts, change port: 9090 → 9092
```

Or find the culprit:

```bash
lsof -iTCP -sTCP:LISTEN -n -P | grep 9090
```

### "Screenshot-safe defaults"

Sensitive headers (`Authorization`, `Cookie`, `Set-Cookie`, `X-API-Key`,
`Proxy-Authorization`) are redacted by default both in the TUI and in MCP
output. For the cURL export, press `y` for the redacted version (default)
or `Y` (shift+y) for a raw version that includes real header values.
The MCP equivalent is `get_request` with `includeSecrets: true` (or
`wait_for_request` with the same flag). Tail is always redacted — tail is
the most screenshot-prone surface.

### Extending the redaction list

If your API uses a custom auth header like `X-Company-Token`, add it to
`extra-sensitive-headers` in `config.toml` rather than overwriting
`sensitive-headers` — that way the defaults stay protected:

```toml
extra-sensitive-headers = ["x-company-token"]
```

---

## 6. Config file (optional)

Rustotron reads `$XDG_CONFIG_HOME/rustotron/config.toml` (falls back to
`~/.config/rustotron/config.toml`). All fields are optional and take
defaults matching `rustotron config show`:

```toml
port = 9090
host = "127.0.0.1"
capacity = 500                       # ring-buffer size, must be ≥ 1
ping-interval-ms = 30000              # must be ≥ 1

# `sensitive-headers` REPLACES the default redaction list. If you set it,
# re-include the defaults you still want redacted. To *add* headers to
# the defaults, use `extra-sensitive-headers` instead (recommended).
#
# sensitive-headers = ["authorization", "cookie", "set-cookie", "x-api-key", "proxy-authorization"]

# Append-only: these are merged with the defaults (case-insensitive).
extra-sensitive-headers = [
  "x-company-token",
]
```

Precedence: CLI flags → `RUSTOTRON_*` env vars → config file → defaults.

```bash
rustotron config show     # prints the effective merged config
rustotron config path     # prints where the file is (exists or not)
```

---

## 7. Verify end-to-end

There's a small shell script in `scripts/e2e-tail-smoke.sh` in the repo
that binds an ephemeral port, fires 5 synthetic `api.response` frames,
and asserts tail emitted 5 ndjson lines. Use it to confirm nothing is
wrong with your local build:

```bash
./scripts/e2e-tail-smoke.sh
# [e2e] PASS — got 5 ndjson lines
```

---

Next: see `docs/mcp.md` for the MCP integration story, or `docs/protocol.md`
for the Reactotron wire-protocol reference.
