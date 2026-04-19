//! Signal handling shared across every subcommand that owns the tokio
//! runtime.
//!
//! The challenge is that Unix (SIGINT / SIGTERM) and Windows (ctrl-c only)
//! diverge, but every surface needs to:
//!   1. wait on either signal (SIGTERM != SIGINT),
//!   2. distinguish SIGINT so `main` can exit 130 (PRD NFR-10),
//!   3. never hang a foreground process on shutdown.
//!
//! `wait_for_shutdown` centralises the select and returns a [`Shutdown`]
//! tag that commands bubble up. `main` converts `Shutdown::Interrupt` into
//! `std::process::exit(130)` so SIGINT is observable from scripts.

use tokio::signal;

/// How a command's signal wait resolved.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Shutdown {
    /// Ctrl+C / SIGINT. Process should exit with code 130.
    Interrupt,
    /// SIGTERM / equivalent. Process should exit 0.
    Terminate,
}

/// Wait for SIGINT or SIGTERM. On non-Unix, only ctrl-c is supported and
/// always maps to `Interrupt`.
pub async fn wait_for_shutdown() -> Shutdown {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal as unix_signal};
        match unix_signal(SignalKind::terminate()) {
            Ok(mut sigterm) => {
                tokio::select! {
                    _ = signal::ctrl_c() => Shutdown::Interrupt,
                    _ = sigterm.recv() => Shutdown::Terminate,
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "could not install SIGTERM handler; Ctrl+C only");
                let _ = signal::ctrl_c().await;
                Shutdown::Interrupt
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = signal::ctrl_c().await;
        Shutdown::Interrupt
    }
}
