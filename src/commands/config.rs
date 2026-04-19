//! `rustotron config` subcommand — inspection only at v1.
//!
//! Writing the config file interactively is out of scope; users edit
//! `$XDG_CONFIG_HOME/rustotron/config.toml` directly. These helpers give
//! them what they need to know what's in effect and where the file lives.

use crate::cli::{Cli, ConfigAction};
use crate::config::{self, CliOverrides};

/// Entry point.
///
/// # Errors
///
/// Propagates [`config::ConfigError`] when the file is malformed.
pub fn run(cli: &Cli, action: &ConfigAction) -> color_eyre::Result<()> {
    match action {
        ConfigAction::Path => {
            println!("{}", config::config_path_display());
        }
        ConfigAction::Show => {
            let overrides = CliOverrides {
                port: cli.port,
                host: cli.host.clone(),
            };
            let cfg = config::load(&overrides)?;
            let ser = toml::to_string(&cfg).map_err(|e| {
                color_eyre::eyre::eyre!("failed to serialise effective config as TOML: {e}")
            })?;
            println!("# rustotron effective configuration");
            println!("# source precedence: CLI > env > file > defaults");
            println!("# file: {}", config::config_path_display());
            print!("{ser}");
        }
    }
    Ok(())
}
