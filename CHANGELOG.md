# Changelog

All notable changes to rustotron will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Initial implementation: WS server (Reactotron-compatible),
  in-memory store actor, broadcast bus.
- Three surfaces: TUI (Ratatui), MCP server (rmcp), tail mode.
- TUI: vim + arrow keybindings, mouse hit-testing, syntect-highlighted
  JSON detail pane, filter bar, pause/clear, cURL export.
- MCP tools: `health`, `list_requests`, `get_request`,
  `search_requests`, `wait_for_request`, `clear_requests`.
- Layered config (CLI → env → file → defaults) via figment.
- `rustotron config show` / `config path` subcommands.
- Reactotron falsy-value sentinel repair (`~~~ null ~~~` etc.).
- Backend-driven pause (store stops accepting inserts, not just TUI).
- Duplicate-`clientId` takeover with 500 ms grace period.
- Friendly `port-in-use` error with `--port` hint and `lsof` recipe.
- TUI mouse-capture runtime toggle (`M`) so users can select text.
- TUI tracing routed to a log file so warnings don't corrupt the
  alt-screen render.
- 185 tests + criterion bench for bus throughput.

## [0.1.0] — TBD

First public release. See `docs/USAGE.md` for the full feature list and
`docs/setup.md` for a 10-minute walkthrough.

[Unreleased]: https://github.com/LEGO-SUDO/rustotron/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/LEGO-SUDO/rustotron/releases/tag/v0.1.0
