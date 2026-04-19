# ADR-001: Dependency choices and packaging

**Status:** Accepted
**Date:** 2026-04-19
**Supersedes:** —

## Context

BUILD_PLAN §2.4 lists a dep stack but flags several choices as "deferred to
Phase 0". This ADR resolves them before `cargo add`, so every downstream task
inherits a known-good set.

## Decisions

### D-1. WebSocket server: `tokio-tungstenite` directly, **not** `axum`

The server's job is narrow: accept WS upgrades on one port, read text frames,
decode to `protocol::Message`, forward. No HTTP routing, no middleware, no
request multiplexing. `axum` would add ~400 KB of HTTP machinery we don't
exercise.

PRD KEY_DETAILS is explicit: _"Use tokio-tungstenite directly. Don't bring in
axum."_ Sticking with that.

Consequence: we hand-write the HTTP upgrade handshake (tungstenite exposes the
low-level API) and accept that browser-style routing is a non-goal. Worth it
for the binary-size savings (NFR-1: ≤ 8 MB stripped).

### D-2. MCP SDK: `rmcp`, **pinned to exact version**

PRD pins the strategy: `rmcp = "=0.x.y"` — exact-equals version, updated via
manual bump PRs.

Rationale: MCP is a spec under active churn (NFR / risk table: "MCP spec
churn — Medium likelihood, Medium impact"). `rmcp` tracks the spec and
occasionally ships behavioural changes inside minor bumps. Pinning exact
versions makes every upgrade a deliberate, reviewable event rather than a
surprise from `cargo update`.

When the first `cargo add rmcp` runs, we capture the then-latest `0.x.y`
and pin it. Future bumps = one PR that updates the pin, reruns the MCP
integration tests, and touches nothing else.

### D-3. Packaging: single crate at v1, split only on concrete triggers

Start as a single `rustotron` crate (lib + bin). Do **not** pre-split into a
workspace.

Split triggers — any one fires, we split:

1. **Compile time** — clean debug build exceeds 30 s on reference hardware
   (M2 Pro), indicating we should separate hot modules from cold.
2. **External reuse** — an external consumer needs `rustotron-protocol` or
   `rustotron-mcp` as their own crate on crates.io.
3. **Feature-flag explosion** — feature combinations cross-contaminate to the
   point where we need separate crates to isolate them (e.g. the TUI pulls
   syntect even in MCP mode).

Rationale: workspaces add meaningful complexity (shared target dir, multiple
Cargo.toml, `--workspace` flag in CI, separate version bumps) for little
payoff at v1. YAGNI applies.

PRD endorses this: _"Start as a single crate. Split only when one of three
concrete triggers fires."_

### D-4. Feature flags: minimal, orthogonal, off-by-default where possible

v1 ships with **no** feature flags on the library surface. All subcommands
(`rustotron`, `rustotron mcp`, `rustotron tail`) ship in the one binary. The
binary-size budget (NFR-1) can absorb them all; conditional compilation
would add complexity without user-visible value.

If v2 introduces heavy-weight optional surfaces (e.g. SQLite persistence), we
revisit with named features.

### D-5. Logging: `tracing` + `tracing-subscriber` with a boundary-specific subscriber

- In TUI and tail modes, tracing writes to stderr at the level set by
  `-v`/`-vv`/`-vvv` (or `RUST_LOG` if set).
- In MCP mode, tracing **must** write to stderr only — stdout is reserved for
  JSON-RPC frames. A single stray log byte on stdout corrupts the protocol
  (NFR-14).
- The subscriber is installed exactly once in `main.rs` before any async
  runtime starts. The MCP subcommand picks a stderr-only subscriber; other
  subcommands pick the default stderr subscriber with color.

### D-6. Error handling: `thiserror` in library code, `color-eyre` in binary code

- Library modules (`protocol`, `server`, `store`, `bus`, `mcp`) expose
  `Result<T, E>` where `E` is a `thiserror`-derived enum. Never `Box<dyn Error>`.
  Never panic in library code.
- Binary layer (`main.rs`, `commands/*`) uses `color_eyre::Result<T>` so the
  user sees pretty backtraces and suggestion hints (e.g. port-in-use).
- `color_eyre::install()` runs before any other code in `main`, so panics
  get the good report.

### D-7. Config: `figment` with TOML + env providers, XDG paths via `directories-next`

Precedence (highest first): CLI flags → `RUSTOTRON_*` env vars → config file
→ built-in defaults. Matches FR-23.

Config file path: `$XDG_CONFIG_HOME/rustotron/config.toml`, fallback
`~/.config/rustotron/config.toml`. Resolved via `directories-next::ProjectDirs`.

## Accepted dependency set (v1)

Hot dependencies (library + binary):

| Crate | Feature set | Why |
|---|---|---|
| `tokio` | `full` | runtime, fs, net, sync, time, macros, signal |
| `tokio-util` | `rt` | `CancellationToken` for graceful shutdown |
| `tokio-tungstenite` | default | WS server (D-1) |
| `serde` | `derive` | protocol structs + config |
| `serde_json` | default | protocol payloads |
| `thiserror` | default | library errors (D-6) |
| `color-eyre` | default | binary errors (D-6) |
| `tracing` | default | structured logging (D-5) |
| `tracing-subscriber` | `env-filter`, `fmt` | `RUST_LOG`, stderr writer |
| `clap` | `derive` | CLI |
| `figment` | `toml`, `env` | layered config (D-7) |
| `directories-next` | default | XDG paths (D-7) |
| `ratatui` | default | TUI |
| `crossterm` | `event-stream` | terminal backend + async events |
| `syntect` | `default-fancy` | JSON highlighting in detail pane |
| `rmcp` | `server` (exact pin) | MCP SDK (D-2) |
| `schemars` | default | MCP tool JSON schemas |
| `uuid` | `v4`, `serde` | requestId shapes in protocol |

Dev dependencies:

| Crate | Why |
|---|---|
| `insta` | TUI snapshot tests |
| `tokio-test` | `async` testing helpers |
| `criterion` | store + bus throughput benches |
| `pretty_assertions` | diff output for fixture replay tests |

## Consequences

- `axum` and `hyper` stay out of the tree → smaller binary.
- `rmcp` upgrades are never silent → MCP behaviour doesn't drift between
  commits.
- No workspace overhead at v1. First split (if any) is a deliberate Phase 5+
  call.
- Config story is consistent across surfaces — no bespoke env parsing in
  subcommands.
