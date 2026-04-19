//! WebSocket server runtime configuration.

use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

/// Default port per PRD FR-1 (9090 is Reactotron's; 9091 coexists).
pub const DEFAULT_PORT: u16 = 9091;

/// Default keepalive cadence per ADR-003 §F-9.
pub const DEFAULT_PING_INTERVAL: Duration = Duration::from_secs(30);

/// Configuration handed to [`super::run`] at startup.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Address to bind on. `127.0.0.1` by default — users opt into exposing
    /// the server on all interfaces (LAN debugging from a device on Wi-Fi)
    /// via config.
    pub host: IpAddr,
    /// TCP port. Default 9091.
    pub port: u16,
    /// How often the server sends WebSocket pings. We never fail a session
    /// on missing pongs (older Reactotron clients don't pong) — pings are
    /// only there to keep intermediate proxies happy.
    pub ping_interval: Duration,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: DEFAULT_PORT,
            ping_interval: DEFAULT_PING_INTERVAL,
        }
    }
}

impl ServerConfig {
    /// Shortcut used in tests: bind to `127.0.0.1:0` so the OS picks a port.
    #[must_use]
    pub fn ephemeral() -> Self {
        Self {
            port: 0,
            ..Self::default()
        }
    }
}
