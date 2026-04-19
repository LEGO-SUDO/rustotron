# rustotron

Terminal-native network inspector for React Native apps. Wire-compatible with
[Reactotron](https://github.com/infinitered/reactotron)'s client, exposing the
same event stream to a Ratatui TUI and an MCP server for AI coding agents.

> **Status:** pre-alpha. See `docs/PRD.md` and `docs/BUILD_PLAN.md` for the full
> spec. Not yet installable.

## What it does (at v0.1.0)

- Launch `rustotron` in a terminal pane → see live HTTP requests from your RN
  app, without alt-tabbing to an Electron window.
- `rustotron mcp` → speak MCP over stdio so Claude Code / Cursor can
  `wait_for_request`, `list_requests`, `get_request` against live traffic.
- `rustotron tail` → one request per line to stdout, pipeable to `grep` / `jq` /
  `fzf`.
- Zero app-side code changes: requires only
  `reactotron-react-native` + the networking plugin in the target RN app.

## Non-goals (v1)

- Not a Redux/state inspector (v1.2)
- Not a console log viewer (v1.1)
- Not a source-level debugger — React Native DevTools owns that space
- Not an HTTP proxy (mitmproxy-style) — only observes what the RN app reports

## Install

_TBD — see the build plan, Phase 4. Homebrew, `cargo install`, `curl | sh`, and
binary downloads shipped at v0.1.0._

## License

Dual-licensed under either of

- Apache License 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.

## Credits

Protocol compatibility is built by reading
[Reactotron](https://github.com/infinitered/reactotron) by Infinite Red.
This is an independent reimplementation of the client-server protocol — no
Reactotron code is bundled.
