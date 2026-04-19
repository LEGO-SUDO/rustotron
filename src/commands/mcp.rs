//! `rustotron mcp` subcommand — stdio MCP server.
//!
//! Runs the standard backend (bus + store + WS server listening for RN
//! clients) and simultaneously serves MCP over stdio so AI agents can
//! query it. The TUI is not started — stdout is reserved for JSON-RPC.

use std::time::Duration;

use rmcp::{ServiceExt, transport::stdio};
use tokio_util::sync::CancellationToken;

use crate::bus::new_bus;
use crate::config::{self, CliOverrides};
use crate::mcp::RustotronMcp;
use crate::server::{self, ServerConfig};
use crate::store::{self, StoreConfig};

use super::signals::{Shutdown, wait_for_shutdown};

/// Entrypoint for `rustotron mcp`. Runs until the MCP peer disconnects
/// (stdio closes) or SIGINT is received.
///
/// # Errors
///
/// Propagates server bind errors. rmcp serve errors are logged to stderr
/// and bubbled.
pub async fn run(host: Option<String>, port: Option<u16>) -> color_eyre::Result<Option<Shutdown>> {
    let token = CancellationToken::new();
    let bus = new_bus(1024);

    let cfg =
        config::load(&CliOverrides { port, host }).map_err(|e| color_eyre::eyre::eyre!("{e}"))?;
    let server_config = ServerConfig {
        host: cfg
            .host
            .parse()
            .map_err(|e| color_eyre::eyre::eyre!("invalid host {host:?}: {e}", host = cfg.host))?,
        port: cfg.port,
        ping_interval: std::time::Duration::from_millis(cfg.ping_interval_ms),
    };
    let store_config = StoreConfig {
        capacity: cfg.capacity,
        sensitive_headers: cfg.effective_sensitive_headers(),
    };

    let store_task = store::spawn(store_config, bus.clone(), token.clone());
    let store_handle = store_task.handle.clone();

    // Bind WS before spawning so bind errors surface immediately (and
    // before we start writing JSON-RPC to stdout).
    let bound = server::bind(&server_config).await?;
    tracing::info!(addr = %bound.local_addr(), "rustotron WS server bound (MCP mode)");

    let server_join = {
        let config = server_config.clone();
        let store = store_handle.clone();
        let bus = bus.clone();
        let token = token.clone();
        tokio::spawn(async move {
            server::serve(bound, config, store, bus, token).await;
        })
    };

    // Run the MCP service on stdio. `serve` returns a handle; `waiting()`
    // blocks until the transport closes.
    let service = RustotronMcp::new(store_handle.clone(), bus.clone());
    let service = service
        .serve(stdio())
        .await
        .map_err(|e| color_eyre::eyre::eyre!("mcp serve failed: {e}"))?;

    let outcome = tokio::select! {
        _ = service.waiting() => {
            tracing::info!("mcp peer disconnected");
            None
        }
        s = wait_for_shutdown() => {
            tracing::info!(?s, "shutdown signal received");
            Some(s)
        }
    };

    token.cancel();
    let _ = tokio::time::timeout(Duration::from_millis(750), server_join).await;
    let _ = tokio::time::timeout(Duration::from_millis(500), store_task.join).await;
    Ok(outcome)
}
