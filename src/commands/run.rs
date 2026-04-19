//! Default subcommand: launches WS server + TUI.
//!
//! Wires the canonical backend pipeline (bus → store → WS server) against
//! the TUI surface, all sharing a single [`CancellationToken`]. When
//! `--mock` is set, skips the WS server and pre-populates the store with
//! 20 fixture rows so the TUI renders meaningful content without any
//! connected RN client.

use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::bus::new_bus;
use crate::cli::Cli;
use crate::config::{self, CliOverrides};
use crate::server::{self, ServerConfig};
use crate::store::{self, StoreConfig};
use crate::tui::{self, TuiConfig, mock};

use super::signals::{Shutdown, wait_for_shutdown};

/// Entrypoint for `rustotron` (no subcommand). Runs until the TUI exits
/// (user quit) or a signal is received.
///
/// # Errors
///
/// Propagates server-bind errors (invalid host, port in use). Any runtime
/// error inside the TUI or server is logged and bubbled up through the
/// TUI's `Result`.
pub async fn run(cli: &Cli) -> color_eyre::Result<Option<Shutdown>> {
    let token = CancellationToken::new();
    let bus = new_bus(1024);

    let cfg = config::load(&CliOverrides {
        port: cli.port,
        host: cli.host.clone(),
    })
    .map_err(|e| color_eyre::eyre::eyre!("{e}"))?;
    let effective_headers = cfg.effective_sensitive_headers();
    let store_config = StoreConfig {
        capacity: cfg.capacity,
        sensitive_headers: effective_headers.clone(),
    };

    let store_task = store::spawn(store_config, bus.clone(), token.clone());
    let store_handle = store_task.handle.clone();

    // Decide whether to run with a live WS server or mock data.
    if cli.mock {
        for exchange in mock::mock_exchanges() {
            if let Err(e) = store_handle.on_response(exchange, None).await {
                tracing::warn!(error = %e, "failed to seed mock row");
            }
        }
        let tui_config = TuiConfig::mock().with_sensitive_headers(cfg.sensitive_headers.clone());

        // Run the TUI alongside a signal-listening task. Either can
        // trigger shutdown.
        let tui_token = token.clone();
        let tui_handle = {
            let store = store_handle.clone();
            let bus = bus.clone();
            tokio::spawn(async move { tui::run(tui_config, store, bus, tui_token).await })
        };

        let outcome = wait_for_signal_or_tui(&token, tui_handle).await?;

        token.cancel();
        let _ = tokio::time::timeout(Duration::from_millis(500), store_task.join).await;
        return Ok(outcome);
    }

    // Live mode: bind WS server first.
    let server_config = ServerConfig {
        host: cfg
            .host
            .parse()
            .map_err(|e| color_eyre::eyre::eyre!("invalid host {host:?}: {e}", host = cfg.host))?,
        port: cfg.port,
        ping_interval: Duration::from_millis(cfg.ping_interval_ms),
    };
    let bound = server::bind(&server_config)
        .await
        .map_err(friendly_bind_err)?;
    let addr = bound.local_addr();

    let server_join = {
        let config = server_config.clone();
        let store = store_handle.clone();
        let bus = bus.clone();
        let token = token.clone();
        tokio::spawn(async move {
            server::serve(bound, config, store, bus, token).await;
        })
    };

    let tui_config =
        TuiConfig::live(format!("ws://{addr}")).with_sensitive_headers(effective_headers.clone());
    let tui_token = token.clone();
    let tui_handle = {
        let store = store_handle.clone();
        let bus = bus.clone();
        tokio::spawn(async move { tui::run(tui_config, store, bus, tui_token).await })
    };

    let outcome = wait_for_signal_or_tui(&token, tui_handle).await?;

    token.cancel();
    let _ = tokio::time::timeout(Duration::from_millis(750), server_join).await;
    let _ = tokio::time::timeout(Duration::from_millis(500), store_task.join).await;
    Ok(outcome)
}

/// Wait until either (a) the TUI exits (user pressed q / Ctrl+C inside
/// the TUI), or (b) a SIGINT / SIGTERM arrives from outside.
async fn wait_for_signal_or_tui(
    token: &CancellationToken,
    tui: tokio::task::JoinHandle<Result<(), tui::TuiError>>,
) -> color_eyre::Result<Option<Shutdown>> {
    tokio::select! {
        biased;
        () = token.cancelled() => Ok(None),
        res = tui => {
            match res {
                Ok(Ok(())) => Ok(None),
                Ok(Err(e)) => Err(color_eyre::eyre::eyre!("tui exited with error: {e}")),
                Err(e) => Err(color_eyre::eyre::eyre!("tui task join failed: {e}")),
            }
        }
        s = wait_for_shutdown() => Ok(Some(s)),
    }
}

/// Turn a bind error into a friendly report.suggest `--port` and show a
/// hint to find the culprit. Matches PRD TASK-301 intent.
fn friendly_bind_err(err: crate::server::ServerError) -> color_eyre::Report {
    match err {
        crate::server::ServerError::Bind { addr, source } => {
            let mut msg = format!("failed to bind on {addr}: {source}");
            if source.kind() == std::io::ErrorKind::AddrInUse {
                msg.push_str(
                    "\n\nhint: another process is listening on that port. Try:\n  \
                     rustotron --port 9092\n\nOr find the culprit with:\n  \
                     lsof -iTCP -sTCP:LISTEN -n -P | grep ",
                );
                msg.push_str(&addr.port().to_string());
            }
            color_eyre::eyre::eyre!(msg)
        }
    }
}
