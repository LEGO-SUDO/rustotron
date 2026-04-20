# Changelog

All notable changes to rustotron will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] — 2026-04-20

First public release. See `docs/USAGE.md` for the full feature list and
`docs/setup.md` for a 10-minute walkthrough.

### Added
- WebSocket server wire-compatible with Reactotron
  (default port `9090`, falsy-sentinel repair, duplicate-clientId takeover
  with 500 ms grace period).
- In-memory store actor + broadcast bus (bounded ring buffer,
  O(1) insert, no `Arc<Mutex<T>>` anywhere on the hot path).
- Three surfaces driven from the same store:
  - **TUI** (Ratatui + crossterm): vim + arrow keybindings, mouse
    hit-testing, syntect-highlighted JSON detail pane, filter bar,
    pause/clear, per-row cURL copy.
  - **MCP server** (rmcp 1.5.0): `health`, `list_requests`,
    `get_request`, `search_requests`, `wait_for_request`,
    `clear_requests` tools for AI agents.
  - **Tail mode**: NDJSON stream on stdout for Unix pipelines.
- Compact-JSON body storage with 256 KB cap and lazy `as_value()`
  parse — keeps RSS bounded on multi-MB responses.
- Layered config (CLI → env → file → defaults) via figment,
  with `rustotron config show` / `config path` subcommands.
- Backend-driven pause (store stops accepting inserts, not just TUI).
- Friendly `port-in-use` error with `--port` hint and `lsof` recipe.
- TUI mouse-capture runtime toggle (`M`) so users can select text
  in the detail pane without leaving the TUI.
- TUI tracing routed to a log file in the OS data dir so warnings
  don't corrupt the alt-screen render.
- Status-bar toast feedback on copy, plus RAM usage chip.
- 185 tests + criterion benchmark for bus throughput.

[Unreleased]: https://github.com/LEGO-SUDO/rustotron/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/LEGO-SUDO/rustotron/releases/tag/v0.1.0
