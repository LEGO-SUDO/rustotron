//! `rustotron tail` — one completed request per line to stdout.
//!
//! Architecture: spin up the standard backend (bus + store + WS server),
//! then subscribe to the bus and print a summary line for every
//! `ResponseReceived` event. Respects `--json` for ndjson output and
//! `--color auto|always|never` plus `NO_COLOR` for ANSI.
//!
//! Stdout is reserved for payload data (lines or ndjson). All logs,
//! banners, diagnostics go to stderr via `tracing`.

use std::io::{self, Write};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;

use crate::bus::{Event, EventBus, RequestId, new_bus};
use crate::cli::{ColorMode, TailArgs};
use crate::config::{self, CliOverrides};
use crate::server::{self, ServerConfig};
use crate::store::{self, Request, SecretsMode, StoreConfig};

use super::signals::{Shutdown, wait_for_shutdown};

/// Top-level tail entry point. `host` and `port` override the defaults
/// when set by global flags (TASK-302 will layer config on top).
///
/// # Errors
///
/// Propagates server bind errors. Any other failure during streaming is
/// logged at `warn` and the tail task keeps running.
pub async fn run(
    args: &TailArgs,
    host: Option<String>,
    port: Option<u16>,
    color: ColorMode,
) -> color_eyre::Result<Option<Shutdown>> {
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

    // Bind before spawning so bind errors are reported synchronously.
    let bound = server::bind(&server_config).await?;
    let local_addr = bound.local_addr();
    eprintln!("rustotron: listening on ws://{local_addr}, press ctrl-c to exit");

    let server_join = {
        let config = server_config.clone();
        let store = store_handle.clone();
        let bus = bus.clone();
        let token = token.clone();
        tokio::spawn(async move {
            server::serve(bound, config, store, bus, token).await;
        })
    };

    let json = args.json;
    let color = args.color_from_global(color);
    let printer_token = token.clone();
    let printer_bus = bus.clone();
    let printer_store = store_handle.clone();
    let printer_join = tokio::spawn(async move {
        run_printer(printer_bus, printer_store, printer_token, json, color).await;
    });

    // Wait for SIGINT / SIGTERM.
    let outcome = wait_for_shutdown().await;
    eprintln!("rustotron: shutting down");
    token.cancel();

    let _ = tokio::time::timeout(Duration::from_millis(750), server_join).await;
    let _ = tokio::time::timeout(Duration::from_millis(500), printer_join).await;
    let _ = tokio::time::timeout(Duration::from_millis(500), store_task.join).await;
    Ok(Some(outcome))
}

async fn run_printer(
    bus: EventBus,
    store: store::StoreHandle,
    token: CancellationToken,
    json: bool,
    color: bool,
) {
    let mut rx = bus.subscribe();

    loop {
        tokio::select! {
            biased;
            () = token.cancelled() => break,
            ev = rx.recv() => match ev {
                Ok(Event::ResponseReceived(id)) => {
                    if let Err(e) = emit_row(&store, id, json, color).await {
                        tracing::warn!(error = %e, "tail emit failed");
                    }
                }
                Ok(_) => {}
                Err(RecvError::Closed) => break,
                Err(RecvError::Lagged(n)) => {
                    tracing::warn!(events_missed = n, "tail subscriber lagged");
                }
            }
        }
    }
}

async fn emit_row(
    store: &store::StoreHandle,
    id: RequestId,
    json: bool,
    color: bool,
) -> io::Result<()> {
    // Redacted by default — tail output is the most likely to be pasted
    // into a chat or screenshot.
    let row = match store.get(id, SecretsMode::Redacted).await {
        Ok(Some(r)) => r,
        Ok(None) => return Ok(()), // evicted before we got here
        Err(e) => {
            tracing::warn!(error = %e, "store unavailable from tail");
            return Ok(());
        }
    };

    // Acquire the stdout lock only for the duration of a single line —
    // lock guards are not Send, so we must never hold across an await.
    let mut out = io::stdout().lock();
    if json {
        write_ndjson(&mut out, &row)
    } else {
        write_columns(&mut out, &row, color)
    }
}

fn write_ndjson(out: &mut impl Write, row: &Request) -> io::Result<()> {
    let line = serde_json::json!({
        "id": row.id.to_string(),
        "received_at_ms": system_time_to_epoch_ms(row.received_at),
        "method": row.exchange.request.method,
        "url": row.exchange.request.url,
        "status": row.exchange.response.status,
        "duration_ms": row.exchange.duration,
    });
    writeln!(out, "{line}")
}

fn write_columns(out: &mut impl Write, row: &Request, color: bool) -> io::Result<()> {
    let hhmmss = format_time_hhmmss(row.received_at);
    let method = row.exchange.request.method.as_deref().unwrap_or("???");
    let status = row.exchange.response.status;
    let duration = row
        .exchange
        .duration
        .map_or_else(|| "    ?".to_string(), |d| format!("{d:.0}ms"));
    let path = url_path_query(&row.exchange.request.url);

    if color {
        writeln!(
            out,
            "{hhmmss} {method:<6} {status_colored} {duration:>6} {path}",
            status_colored = colorise_status(status),
        )
    } else {
        writeln!(out, "{hhmmss} {method:<6} {status:<3} {duration:>6} {path}")
    }
}

fn colorise_status(status: u16) -> String {
    // ANSI 16-colour palette, bright variants for legibility over dark bg.
    let code = match status / 100 {
        2 => 32, // green
        3 => 36, // cyan
        4 => 33, // yellow
        5 => 31, // red
        _ => 0,  // default
    };
    format!("\x1b[{code}m{status:<3}\x1b[0m")
}

fn format_time_hhmmss(t: SystemTime) -> String {
    match t.duration_since(UNIX_EPOCH) {
        Ok(dur) => {
            let total = dur.as_secs();
            let h = (total / 3600) % 24;
            let m = (total / 60) % 60;
            let s = total % 60;
            format!("{h:02}:{m:02}:{s:02}")
        }
        Err(_) => "??:??:??".to_string(),
    }
}

fn system_time_to_epoch_ms(t: SystemTime) -> u128 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis())
}

fn url_path_query(url: &str) -> String {
    // "https://host/foo?bar=baz" → "/foo?bar=baz". If we can't find a
    // scheme, show the whole thing.
    if let Some(scheme_end) = url.find("://") {
        let after_scheme = &url[scheme_end + 3..];
        if let Some(path_start) = after_scheme.find('/') {
            return after_scheme[path_start..].to_string();
        }
    }
    url.to_string()
}

// Glue to keep the main CLI dispatch simple: bridges a potentially-absent
// global color mode into a bool.
impl TailArgs {
    /// Decide whether to emit ANSI colours on stdout. `global` comes from
    /// the `--color` top-level flag; `tail` does not define its own.
    #[must_use]
    pub fn color_from_global(&self, global: ColorMode) -> bool {
        // When emitting ndjson we unconditionally omit color (it would
        // corrupt the output). Otherwise defer to the global policy.
        if self.json {
            return false;
        }
        global.enabled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_time_stays_within_24_hour_window() {
        let epoch = SystemTime::UNIX_EPOCH;
        assert_eq!(format_time_hhmmss(epoch), "00:00:00");
        let t = epoch + Duration::from_secs(3600 + 120 + 7);
        assert_eq!(format_time_hhmmss(t), "01:02:07");
    }

    #[test]
    fn url_path_query_strips_scheme_and_host() {
        assert_eq!(url_path_query("https://x.test/a/b?c=1"), "/a/b?c=1");
        assert_eq!(url_path_query("http://host/"), "/");
        assert_eq!(url_path_query("/already-a-path"), "/already-a-path");
    }

    #[test]
    fn colorise_status_wraps_status_with_ansi_when_enabled() {
        let painted = colorise_status(200);
        assert!(painted.contains("\x1b[32m"));
        assert!(painted.contains("200"));
        assert!(painted.ends_with("\x1b[0m"));
    }

    #[test]
    fn tail_args_json_forces_color_off_even_with_always() {
        let args = TailArgs { json: true };
        assert!(!args.color_from_global(ColorMode::Always));
    }

    #[test]
    fn write_columns_produces_expected_shape_with_no_color() {
        let mut buf = Vec::new();
        let row = sample_row("POST", 201, Some(340.0), "https://api.test/tx");
        write_columns(&mut buf, &row, false).unwrap();
        let line = String::from_utf8(buf).unwrap();
        assert!(line.ends_with("/tx\n"));
        assert!(line.contains("POST"));
        assert!(line.contains("201"));
        assert!(line.contains("340ms"));
    }

    #[test]
    fn write_ndjson_emits_valid_json_line() {
        let mut buf = Vec::new();
        let row = sample_row("GET", 200, Some(15.0), "https://api.test/ping");
        write_ndjson(&mut buf, &row).unwrap();
        let line = String::from_utf8(buf).unwrap();
        let v: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(v.get("method").and_then(|m| m.as_str()), Some("GET"));
        assert_eq!(v.get("status").and_then(|s| s.as_u64()), Some(200));
        assert_eq!(v.get("duration_ms").and_then(|d| d.as_f64()), Some(15.0));
    }

    fn sample_row(method: &str, status: u16, duration_ms: Option<f64>, url: &str) -> Request {
        use crate::protocol::{ApiRequestSide, ApiResponsePayload, ApiResponseSide};
        use serde_json::Value;

        let exchange = ApiResponsePayload {
            duration: duration_ms,
            request: ApiRequestSide {
                url: url.to_string(),
                method: Some(method.to_string()),
                data: Value::Null,
                headers: None,
                params: None,
            },
            response: ApiResponseSide {
                status,
                headers: None,
                body: Value::Null,
            },
        };
        Request::complete(exchange, None)
    }
}
