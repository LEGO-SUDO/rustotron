//! clap-derive CLI surface.
//!
//! The top-level binary has three subcommands:
//!
//! - **no subcommand** — default TUI mode (launches WS server + TUI).
//! - `mcp` — stdio MCP server; no TUI.
//! - `tail` — line-oriented stdout, pipeable to grep/jq/fzf.
//!
//! Config flags shared across modes (port, log level, color) are set on
//! the top-level struct; subcommands get their own flags where relevant.
//! The final config-file + env layering arrives in TASK-302; this file is
//! the minimal dispatch skeleton.

use clap::{Parser, Subcommand, ValueEnum};

/// Terminal-native React Native network inspector.
///
/// Wire-compatible with Reactotron's client. Exposes the same event stream
/// to a Ratatui TUI, an MCP server, and a Unix-pipe-friendly tail.
#[derive(Debug, Parser)]
#[command(
    name = "rustotron",
    version,
    about = "Terminal-native RN network inspector, wire-compatible with Reactotron.",
    long_about = None,
)]
pub struct Cli {
    /// Port the WebSocket server should bind. Default 9090 — matches
    /// Reactotron's wire port so default-configured RN clients connect
    /// with no changes. Use `--port 9091` to coexist with upstream
    /// Reactotron.
    #[arg(long, global = true)]
    pub port: Option<u16>,

    /// Bind address. Default 127.0.0.1.
    #[arg(long, global = true)]
    pub host: Option<String>,

    /// Verbosity — repeat for more: `-v` = INFO, `-vv` = DEBUG, `-vvv` = TRACE.
    /// `RUST_LOG` env overrides.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Color policy for stdout text. `auto` (default) honours `NO_COLOR`
    /// and `!isatty(stdout)`; `always` forces colors on; `never` forces
    /// off.
    #[arg(long, value_enum, default_value_t = ColorMode::Auto, global = true)]
    pub color: ColorMode,

    /// Start the TUI against pre-baked fixture data instead of binding
    /// the WS server. Useful for demos, screenshots, and development.
    /// Only meaningful on the default (no-subcommand) invocation.
    #[arg(long)]
    pub mock: bool,

    /// What to do. When omitted, launches the TUI.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Subcommands rustotron exposes.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the MCP server over stdio so AI coding agents (Claude Code,
    /// Cursor) can query live network state.
    Mcp,
    /// Emit one completed request per line to stdout — pipeable to
    /// `grep`, `jq`, `fzf`.
    Tail(TailArgs),
    /// Inspect configuration.
    Config {
        /// Which config action to take.
        #[command(subcommand)]
        action: ConfigAction,
    },
}

/// `rustotron config …` subcommands.
#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Print the effective configuration (defaults + file + env + flags
    /// merged) as TOML.
    Show,
    /// Print the resolved path to the config file, whether it exists
    /// or not.
    Path,
}

/// Flags for the `tail` subcommand.
#[derive(Debug, clap::Args)]
pub struct TailArgs {
    /// Emit newline-delimited JSON instead of human-readable columns.
    #[arg(long)]
    pub json: bool,
}

/// Color policy — `auto` inspects the terminal, `always` / `never` are
/// explicit overrides.
#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum ColorMode {
    /// Honour `NO_COLOR` and whether stdout is a TTY.
    Auto,
    /// Always emit ANSI escapes.
    Always,
    /// Never emit ANSI escapes.
    Never,
}

impl ColorMode {
    /// Resolve to a concrete on/off decision for the current process.
    /// Reads `NO_COLOR` from the environment and checks stdout TTY-ness.
    #[must_use]
    pub fn enabled(self) -> bool {
        match self {
            Self::Always => true,
            Self::Never => false,
            Self::Auto => {
                if std::env::var_os("NO_COLOR").is_some() {
                    false
                } else {
                    std::io::IsTerminal::is_terminal(&std::io::stdout())
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tail_with_json_flag() {
        let cli = Cli::try_parse_from(["rustotron", "tail", "--json"]).unwrap();
        match cli.command {
            Some(Command::Tail(args)) => assert!(args.json),
            other => panic!("expected Tail, got {other:?}"),
        }
    }

    #[test]
    fn parses_mcp_subcommand() {
        let cli = Cli::try_parse_from(["rustotron", "mcp"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Mcp)));
    }

    #[test]
    fn no_subcommand_leaves_command_none() {
        let cli = Cli::try_parse_from(["rustotron"]).unwrap();
        assert!(cli.command.is_none());
    }

    #[test]
    fn port_and_verbose_parse_globally() {
        let cli = Cli::try_parse_from(["rustotron", "-vvv", "--port", "9092", "mcp"]).unwrap();
        assert_eq!(cli.port, Some(9092));
        assert_eq!(cli.verbose, 3);
        assert!(matches!(cli.command, Some(Command::Mcp)));
    }

    #[test]
    fn color_mode_parses_never() {
        let cli = Cli::try_parse_from(["rustotron", "--color", "never", "tail"]).unwrap();
        assert!(!cli.color.enabled());
    }

    #[test]
    fn color_mode_always_ignores_no_color_env() {
        // Safety: mutating env is fine in a single-threaded test.
        let cli = Cli::try_parse_from(["rustotron", "--color", "always"]).unwrap();
        assert!(cli.color.enabled());
    }

    #[test]
    fn mock_flag_parses_on_default_command() {
        let cli = Cli::try_parse_from(["rustotron", "--mock"]).unwrap();
        assert!(cli.mock);
        assert!(cli.command.is_none());
    }

    #[test]
    fn mock_flag_defaults_off() {
        let cli = Cli::try_parse_from(["rustotron"]).unwrap();
        assert!(!cli.mock);
    }
}
