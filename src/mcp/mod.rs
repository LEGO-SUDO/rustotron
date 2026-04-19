//! MCP server surface.
//!
//! Exposed via `rustotron mcp`. Speaks JSON-RPC over stdio using `rmcp`.
//! Tools let AI coding agents (Claude Code, Cursor) observe live RN
//! traffic without copy-paste.
//!
//! See `docs/decisions/003-protocol-reality.md` for why the set of tools
//! is what it is (notably: no `requestId` lives on the RN wire, so ids
//! surfaced here are rustotron-generated UUIDs).

pub mod schema;
pub mod server;
pub mod tools;

pub use self::server::RustotronMcp;
