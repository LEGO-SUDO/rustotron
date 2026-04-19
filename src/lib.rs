//! rustotron — library surface.
//!
//! Re-exports the modules that integration tests (`tests/`) and benches
//! (`benches/`) depend on. The binary (`src/main.rs`) also consumes this
//! library rather than re-declaring its own module tree.

pub mod bus;
pub mod cli;
pub mod commands;
pub mod config;
pub mod mcp;
pub mod protocol;
pub mod server;
pub mod store;
pub mod tui;
