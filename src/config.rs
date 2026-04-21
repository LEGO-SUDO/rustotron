//! Layered configuration.
//!
//! Precedence (highest → lowest): CLI flags → `RUSTOTRON_*` env vars →
//! `$XDG_CONFIG_HOME/rustotron/config.toml` → built-in defaults. Matches
//! PRD FR-23.
//!
//! The merged result is a [`Config`] — a plain `serde` struct. Individual
//! surfaces (server, store, tui) pick the fields they need.

use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;

use directories_next::ProjectDirs;
use figment::{
    Figment,
    providers::{Env, Format, Serialized, Toml},
};
use serde::{Deserialize, Serialize};

use crate::server::{DEFAULT_PING_INTERVAL, DEFAULT_PORT};
use crate::store::{DEFAULT_CAPACITY, default_sensitive_headers};

/// The complete configuration surface rustotron honours.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", default)]
pub struct Config {
    /// WS bind port. Default 9090 — same as Reactotron, so a default
    /// `Reactotron.configure()` call in an RN app connects with zero
    /// changes. Use `--port 9091` to run side-by-side with Reactotron.
    pub port: u16,
    /// WS bind host. Default 127.0.0.1.
    pub host: String,
    /// Ring-buffer capacity for the store actor. Default 500.
    /// Must be ≥ 1 — validated at load time.
    pub capacity: usize,
    /// **Full replacement** of the sensitive-header redaction list. Setting
    /// this means you accept responsibility for re-listing the defaults
    /// (`Authorization`, `Cookie`, `Set-Cookie`, `X-API-Key`,
    /// `Proxy-Authorization`). If you only want to add custom headers to
    /// the defaults, use `extra_sensitive_headers` instead.
    pub sensitive_headers: Vec<String>,
    /// Append-only extension of the default sensitive-header list. Your
    /// entries are merged (case-insensitive deduplication) with the
    /// built-in defaults — so setting `extra-sensitive-headers =
    /// ["x-company-token"]` keeps `Authorization` + friends redacted and
    /// adds your custom header.
    #[serde(default)]
    pub extra_sensitive_headers: Vec<String>,
    /// WebSocket keepalive cadence in milliseconds. Default 30_000
    /// (30 s, matches upstream Reactotron server). Must be ≥ 1.
    pub ping_interval_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            host: Ipv4Addr::LOCALHOST.to_string(),
            capacity: DEFAULT_CAPACITY,
            sensitive_headers: default_sensitive_headers(),
            extra_sensitive_headers: Vec::new(),
            ping_interval_ms: u64::try_from(DEFAULT_PING_INTERVAL.as_millis()).unwrap_or(30_000),
        }
    }
}

impl Config {
    /// Return the effective (merged + deduped, case-insensitive)
    /// sensitive-header list that the store + cURL exporter should use.
    #[must_use]
    pub fn effective_sensitive_headers(&self) -> Vec<String> {
        let mut out = self.sensitive_headers.clone();
        for extra in &self.extra_sensitive_headers {
            if !out.iter().any(|h| h.eq_ignore_ascii_case(extra)) {
                out.push(extra.clone());
            }
        }
        out
    }
}

/// CLI flag values that should override file + env when present. All
/// fields are `Option` so "unset" carries through the figment layering.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CliOverrides {
    /// `--port` flag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// `--host` flag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

/// Errors returned by [`load`]. `figment::Error` is boxed because it
/// carries a large span tree that would otherwise balloon `Result` size.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// Config file exists but can't be parsed as TOML / doesn't match the
    /// schema.
    #[error("config error: {0}")]
    Figment(#[from] Box<figment::Error>),
    /// Resolved host string couldn't be parsed as an IP address.
    #[error("invalid host {host:?}: {source}")]
    InvalidHost {
        /// The string we tried to parse.
        host: String,
        /// Underlying parse error.
        #[source]
        source: std::net::AddrParseError,
    },
    /// A numeric field is out of its allowed range.
    #[error("invalid config: {field} must be ≥ 1 (got {value})")]
    InvalidRange {
        /// Which field was out of range.
        field: &'static str,
        /// The offending value.
        value: u64,
    },
}

/// Load the merged config.
///
/// # Errors
///
/// Returns [`ConfigError::Figment`] when the config file is present but
/// malformed. Missing config file is **not** an error — defaults are used.
pub fn load(cli: &CliOverrides) -> Result<Config, ConfigError> {
    let mut figment = Figment::from(Serialized::defaults(Config::default()));

    if let Some(path) = config_path()
        && path.exists()
    {
        figment = figment.merge(Toml::file(&path));
    }

    figment = figment.merge(Env::prefixed("RUSTOTRON_").split("__"));
    figment = figment.merge(Serialized::defaults(cli));

    let config: Config = figment.extract().map_err(Box::new)?;
    // Sanity-check the host now so later surfaces don't need to.
    if let Err(e) = config.host.parse::<IpAddr>() {
        return Err(ConfigError::InvalidHost {
            host: config.host.clone(),
            source: e,
        });
    }
    if config.capacity == 0 {
        return Err(ConfigError::InvalidRange {
            field: "capacity",
            value: 0,
        });
    }
    if config.ping_interval_ms == 0 {
        return Err(ConfigError::InvalidRange {
            field: "ping-interval-ms",
            value: 0,
        });
    }
    Ok(config)
}

/// Resolve the config-file path following XDG. Returns `None` only if the
/// platform cannot produce a home directory (unusual — CI containers).
#[must_use]
pub fn config_path() -> Option<PathBuf> {
    ProjectDirs::from("dev", "rustotron", "rustotron")
        .map(|dirs| dirs.config_dir().join("config.toml"))
}

/// Path that `rustotron config path` prints. Falls back to a friendly
/// "(unresolved)" marker when the platform can't give us a home dir.
#[must_use]
pub fn config_path_display() -> String {
    config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(platform home directory unresolved)".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_matches_spec() {
        let cfg = Config::default();
        assert_eq!(cfg.port, 9090);
        assert_eq!(cfg.host, "127.0.0.1");
        assert_eq!(cfg.capacity, 500);
        assert!(
            cfg.sensitive_headers
                .iter()
                .any(|h| h.eq_ignore_ascii_case("authorization"))
        );
        assert_eq!(cfg.ping_interval_ms, 30_000);
    }

    #[test]
    fn cli_overrides_defaults() {
        let cli = CliOverrides {
            port: Some(9092),
            host: None,
        };
        let cfg = load(&cli).expect("load");
        assert_eq!(cfg.port, 9092, "CLI should override default port");
        assert_eq!(
            cfg.host, "127.0.0.1",
            "unset CLI host falls back to default"
        );
    }

    #[test]
    fn cli_host_takes_precedence() {
        let cli = CliOverrides {
            port: None,
            host: Some("0.0.0.0".to_string()),
        };
        let cfg = load(&cli).expect("load");
        assert_eq!(cfg.host, "0.0.0.0");
    }

    #[test]
    fn invalid_host_surfaces_error() {
        let cli = CliOverrides {
            port: None,
            host: Some("not-an-ip".to_string()),
        };
        match load(&cli) {
            Err(ConfigError::InvalidHost { host, .. }) => {
                assert_eq!(host, "not-an-ip");
            }
            other => panic!("expected InvalidHost, got {other:?}"),
        }
    }

    #[test]
    fn toml_roundtrips_via_serde() {
        let original = Config::default();
        let ser = toml::to_string(&original).unwrap();
        let parsed: Config = toml::from_str(&ser).unwrap();
        assert_eq!(original, parsed);
    }
}
