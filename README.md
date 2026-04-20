# rustotron

[![CI](https://github.com/LEGO-SUDO/rustotron/actions/workflows/ci.yml/badge.svg)](https://github.com/LEGO-SUDO/rustotron/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/rustotron.svg)](https://crates.io/crates/rustotron)
[![Downloads](https://img.shields.io/crates/d/rustotron.svg)](https://crates.io/crates/rustotron)
[![License](https://img.shields.io/crates/l/rustotron.svg)](#license)
[![MSRV](https://img.shields.io/badge/MSRV-1.85-blue.svg)](#)

> **Terminal-native network inspector for React Native apps.** Wire-compatible
> with [Reactotron](https://github.com/infinitered/reactotron) — point your
> RN app at it, watch HTTP traffic in your terminal, never alt-tab to an
> Electron window again.

<!-- TODO: replace with vhs-generated demo gif (TASK-405) -->
<p align="center">
  <em>(demo gif lands with v0.1.0 — generated via <a href="https://github.com/charmbracelet/vhs">vhs</a>)</em>
</p>

## Why

Reactotron is great. But it's an Electron app, and if you live in tmux + Neovim
+ Ghostty, leaving your keyboard for an entire window manager just to inspect
a single API call is friction. rustotron speaks Reactotron's wire protocol so
your existing RN clients work unchanged — but it renders in a terminal pane,
streams to stdout for `grep`/`jq`, and exposes the same data over MCP so AI
coding agents can reason about live network state without copy-paste.

## Three surfaces, one binary

| Mode | Command | What it does |
|---|---|---|
| TUI | `rustotron` | Three-pane Ratatui UI (request list, JSON detail, status bar). vim + arrows + mouse. |
| Tail | `rustotron tail` / `tail --json` | One request per line to stdout. Pipe to `grep`, `jq`, `fzf`. |
| MCP | `rustotron mcp` | JSON-RPC over stdio. Plugs into Claude Code / Cursor. |

## Install

> **v0.1.0 not released yet.** The instructions below are how installation will
> work at launch. For now, build from source: `cargo build --release` (see
> [docs/USAGE.md](docs/USAGE.md)).

```bash
# Homebrew (macOS / Linux)
brew install LEGO-SUDO/rustotron/rustotron

# crates.io (any platform with Rust)
cargo install rustotron

# curl-pipe-sh installer
curl -fsSL https://github.com/LEGO-SUDO/rustotron/releases/latest/download/rustotron-installer.sh | sh

# Or grab a prebuilt binary from the latest release:
# https://github.com/LEGO-SUDO/rustotron/releases
```

## Wire up your RN app — three lines

```bash
cd my-rn-app
npm install reactotron-react-native
```

Create `src/ReactotronConfig.ts` and import it once in your entrypoint:

```ts
import Reactotron, { networking } from 'reactotron-react-native';

Reactotron
  .configure({ name: 'my-app', host: '127.0.0.1', port: 9090 })
  .use(networking())
  .connect();
```

Run `rustotron` in one terminal, your RN app in another. Done. Full guide in
[docs/setup.md](docs/setup.md); a working Expo example sits at
[`examples/rn-app/`](examples/rn-app/).

## MCP quickstart — for AI coding agents

The flagship tool is `wait_for_request` — agents can block on a specific
in-flight pattern instead of polling logs:

> Agent: `wait_for_request(urlPattern="auth/login", method="POST", timeoutMs=15000)`
> → blocks until the user tries to log in
> → returns the full exchange (redacted by default; pass `includeSecrets=true` for raw)

### Claude Code

Add to `~/.claude/mcp.json`:

```json
{
  "mcpServers": {
    "rustotron": {
      "command": "/usr/local/bin/rustotron",
      "args": ["mcp"]
    }
  }
}
```

### Cursor

Same JSON shape, in Cursor's MCP settings UI. Restart the editor after.

Tools exposed: `health`, `list_requests`, `get_request`, `search_requests`,
`wait_for_request`, `clear_requests`. All filters compose; sensitive headers
redacted by default with explicit `includeSecrets` opt-in.

## How does this compare to Reactotron?

Honest version — it's a different shape, not a replacement. You may want both.

| | rustotron | Reactotron (Electron app) |
|---|---|---|
| Distribution | Single binary, ~5 MB | Electron, ~150 MB |
| UI | Terminal (Ratatui) | Native window (React) |
| Network inspection | ✅ | ✅ |
| Redux / state inspection | ❌ (v1.2 roadmap) | ✅ |
| Console log viewer | ❌ (v1.1 roadmap) | ✅ |
| MCP for AI agents | ✅ | ❌ |
| Unix-pipe output | ✅ (`tail --json`) | ❌ |
| Coexists with the other | ⚠ same default port (9090); run one with `--port 9091` to pair them | ✅ |

If you want full Reactotron — keep using it. If you mostly look at network
traffic and want it in a terminal pane next to your editor + an MCP surface
for your AI assistant, rustotron is for you. Many developers will run both.

## Architecture in one paragraph

Single Rust binary. `tokio-tungstenite` accepts WS connections from the
Reactotron client. A store actor (one tokio task, no shared state) owns a
ring buffer of recent requests. A `tokio::sync::broadcast` channel fans
domain events to subscribers — the TUI, the MCP server, the tail printer.
No `Arc<Mutex<T>>` anywhere in hot paths; the design is documented in
[`docs/decisions/002-concurrency-model.md`](docs/decisions/002-concurrency-model.md).
The Reactotron wire protocol reference is at
[`docs/protocol.md`](docs/protocol.md) for anyone implementing a compatible
client.

## Documentation

- **[docs/USAGE.md](docs/USAGE.md)** — every command, every flag, every keybinding.
- **[docs/setup.md](docs/setup.md)** — first-time setup walkthrough.
- **[docs/protocol.md](docs/protocol.md)** — Reactotron wire protocol reference.
- **[docs/decisions/](docs/decisions/)** — ADRs (deps, concurrency model, protocol reality).

## Contributing

Pull requests welcome. Two non-negotiables:

- Every PR passes `cargo fmt --check` and `cargo clippy --all-targets --all-features -- -D warnings`.
- Every behaviour change includes a test.

The codebase follows an actor-pattern + channels concurrency model.
Don't reach for `Arc<Mutex<T>>` — `.clippy.toml` actively forbids the
`std::sync` blocking primitives and the design rationale is in ADR-002.

```bash
git clone https://github.com/LEGO-SUDO/rustotron
cd rustotron
just lint           # fmt --check + clippy -D warnings
just test           # full suite — 185+ tests
just run -- --mock  # spin up the TUI with fixture data
```

## License

Dual-licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option. Standard Rust convention.

## Credits

- **Infinite Red** for [Reactotron](https://github.com/infinitered/reactotron),
  the wire protocol this tool reuses, and a decade of debugging tools that
  made RN development bearable. rustotron is an independent reimplementation
  of the client-server protocol — no Reactotron source is bundled. If you
  enjoy this, try Reactotron too — different shape, also worth your time.
- The [Ratatui](https://ratatui.rs/) maintainers for the TUI substrate.
- The [Model Context Protocol](https://modelcontextprotocol.io/) team and
  the [`rmcp`](https://crates.io/crates/rmcp) crate authors for making
  agent integration straightforward.
