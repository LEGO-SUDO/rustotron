//! rustotron — terminal-native RN network inspector.
//!
//! Binary entrypoint. Parses CLI, installs tracing (to stderr for
//! subcommands, to a log file for TUI mode so we don't corrupt the
//! alt-screen), then dispatches.

use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};

use clap::Parser;
use directories_next::ProjectDirs;
use rustotron::cli::{Cli, Command};
use rustotron::commands::{self, Shutdown};
use tracing_subscriber::{EnvFilter, fmt};

/// Where tracing goes for a given subcommand. The TUI surface shares
/// a terminal with stderr; any log write scribbles on the alt-screen.
enum TracingSink {
    /// Stderr writer. Fine for subcommands that either don't own the
    /// terminal (MCP uses stdio, tail owns stdout, config is one-shot)
    /// or that the user explicitly launches with redirection.
    Stderr,
    /// File writer. Used in TUI mode so logs never corrupt the render.
    File(PathBuf),
    /// No subscriber installed — we couldn't open the log file and we
    /// refuse to let tracing scribble on the TUI.
    Silent,
}

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();
    let sink = choose_tracing_sink(&cli);
    // `_guard` keeps the tracing-appender non-blocking background worker
    // alive; dropping it flushes the queue. Must live until `main` exits.
    let _guard = install_tracing(cli.verbose, &sink);
    announce_log_file(&sink);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    // Config subcommand does not need the tokio runtime.
    if let Some(Command::Config { ref action }) = cli.command {
        return commands::config::run(&cli, action);
    }

    let outcome: color_eyre::Result<Option<Shutdown>> = runtime.block_on(async move {
        match cli.command {
            None => commands::run::run(&cli).await,
            Some(Command::Tail(ref args)) => {
                commands::tail::run(args, cli.host.clone(), cli.port, cli.color).await
            }
            Some(Command::Mcp) => commands::mcp::run(cli.host.clone(), cli.port).await,
            Some(Command::Config { .. }) => unreachable!("handled above synchronously"),
        }
    });

    // PRD NFR-10: SIGINT exits 130 so shell scripts can tell interrupt
    // apart from normal completion.
    match outcome {
        Ok(Some(Shutdown::Interrupt)) => std::process::exit(130),
        Ok(_) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Decide where tracing output should go for this invocation.
///
/// TUI mode (no subcommand, including `--mock`) routes to a file — the
/// alt-screen shares the terminal with stderr, so even one warn line
/// corrupts the render. Other modes keep stderr.
fn choose_tracing_sink(cli: &Cli) -> TracingSink {
    match cli.command {
        None => match tui_log_path() {
            Some(path) => match ensure_parent_dir(&path) {
                Ok(()) => TracingSink::File(path),
                Err(_) => TracingSink::Silent,
            },
            None => TracingSink::Silent,
        },
        Some(Command::Mcp | Command::Tail(_) | Command::Config { .. }) => TracingSink::Stderr,
    }
}

/// Resolve the TUI-mode log-file path.
/// macOS: `~/Library/Application Support/dev.rustotron.rustotron/rustotron.log`.
/// Linux: `$XDG_DATA_HOME/rustotron/rustotron.log` (via ProjectDirs).
fn tui_log_path() -> Option<PathBuf> {
    ProjectDirs::from("dev", "rustotron", "rustotron")
        .map(|dirs| dirs.data_dir().join("rustotron.log"))
}

fn ensure_parent_dir(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

/// Install the tracing subscriber. Default level is WARN; `-v` / `-vv`
/// / `-vvv` bump to INFO / DEBUG / TRACE; `RUST_LOG` overrides.
///
/// Returns a guard that flushes the background worker on drop (only
/// populated for [`TracingSink::File`] — the non-blocking writer needs
/// its guard to outlive the subscriber).
#[must_use]
fn install_tracing(
    verbose: u8,
    sink: &TracingSink,
) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let default_level = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let filter_for = || {
        EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new(format!("rustotron={default_level}")))
    };

    match sink {
        TracingSink::Stderr => {
            let _ = fmt()
                .with_env_filter(filter_for())
                .with_writer(std::io::stderr)
                .try_init();
            None
        }
        TracingSink::File(path) => {
            let Ok(file) = OpenOptions::new().create(true).append(true).open(path) else {
                return None;
            };
            // `non_blocking` returns a writer that hands writes off to a
            // background worker via an mpsc channel — avoids blocking
            // the TUI render loop on disk I/O and avoids std::sync::Mutex
            // (forbidden by `.clippy.toml`).
            let (writer, guard) = tracing_appender::non_blocking(file);
            let _ = fmt()
                .with_env_filter(filter_for())
                .with_writer(writer)
                .with_ansi(false)
                .try_init();
            Some(guard)
        }
        TracingSink::Silent => None,
    }
}

/// Print the log-file path to stderr ONCE, before the TUI takes over.
fn announce_log_file(sink: &TracingSink) {
    if let TracingSink::File(path) = sink {
        eprintln!("rustotron: logs → {}", path.display());
    }
}
