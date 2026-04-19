//! Store runtime configuration.
//!
//! Kept in its own module so `actor.rs` and `mod.rs` (public API) can
//! share the struct without one importing the other.

use super::redact::default_sensitive_headers;

/// Default ring-buffer capacity (PRD FR-4).
pub const DEFAULT_CAPACITY: usize = 500;

/// Configuration handed to the store at spawn time.
///
/// Typically built from rustotron's layered config (CLI → env → file →
/// defaults) — see TASK-302. Tests construct it directly.
#[derive(Debug, Clone)]
pub struct StoreConfig {
    /// Maximum rows retained in the ring buffer. N+1 insert evicts the
    /// oldest.
    pub capacity: usize,
    /// Case-insensitive header names treated as sensitive in
    /// `SecretsMode::Redacted`. Includes default list plus any user extras.
    pub sensitive_headers: Vec<String>,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            capacity: DEFAULT_CAPACITY,
            sensitive_headers: default_sensitive_headers(),
        }
    }
}

impl StoreConfig {
    /// Construct with a specific capacity; sensitive headers default.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity,
            ..Self::default()
        }
    }
}
