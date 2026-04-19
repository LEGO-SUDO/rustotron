//! Filter state + composition.
//!
//! Composes URL substring, method, and status-class predicates with AND
//! semantics. Lives outside `view::filter` because both the `App` struct
//! and the renderer consume it — view code only owns pixels.

use std::collections::HashSet;

use crate::store::Request;

/// Coarse-grained HTTP status classification used by the filter chips.
#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq)]
pub enum StatusClass {
    /// 2xx
    Success,
    /// 3xx
    Redirect,
    /// 4xx
    ClientError,
    /// 5xx
    ServerError,
}

impl StatusClass {
    /// Map an HTTP status code to its class, if any. Returns `None` for
    /// 1xx / out-of-range codes the chips do not expose.
    #[must_use]
    pub fn from_status(status: u16) -> Option<Self> {
        match status / 100 {
            2 => Some(StatusClass::Success),
            3 => Some(StatusClass::Redirect),
            4 => Some(StatusClass::ClientError),
            5 => Some(StatusClass::ServerError),
            _ => None,
        }
    }

    /// Short display label used by the chip renderer ("2xx", "3xx", …).
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            StatusClass::Success => "2xx",
            StatusClass::Redirect => "3xx",
            StatusClass::ClientError => "4xx",
            StatusClass::ServerError => "5xx",
        }
    }
}

/// The filter a TUI `App` holds. Empty sets / empty string mean "no
/// constraint on that axis" — the filter returns everything.
#[derive(Debug, Clone, Default)]
pub struct FilterState {
    /// Case-insensitive URL substring. Empty string → no URL filter.
    pub url_substring: String,
    /// Uppercase method names (`GET`, `POST`, …). Empty set → any.
    pub methods: HashSet<String>,
    /// Selected status classes. Empty set → any.
    pub status_classes: HashSet<StatusClass>,
}

impl FilterState {
    /// Is this filter currently a no-op (shows every row)?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.url_substring.is_empty() && self.methods.is_empty() && self.status_classes.is_empty()
    }

    /// Does the supplied row satisfy every active predicate?
    #[must_use]
    pub fn matches(&self, req: &Request) -> bool {
        if !self.url_substring.is_empty() {
            let needle = self.url_substring.to_ascii_lowercase();
            if !req
                .exchange
                .request
                .url
                .to_ascii_lowercase()
                .contains(&needle)
            {
                return false;
            }
        }
        if !self.methods.is_empty() {
            let m = req
                .exchange
                .request
                .method
                .as_deref()
                .unwrap_or("")
                .to_ascii_uppercase();
            if !self.methods.contains(&m) {
                return false;
            }
        }
        if !self.status_classes.is_empty() {
            match StatusClass::from_status(req.exchange.response.status) {
                Some(class) => {
                    if !self.status_classes.contains(&class) {
                        return false;
                    }
                }
                None => return false,
            }
        }
        true
    }
}

/// Toggle presence of `value` in `set` — removes if present, inserts
/// otherwise. Convenience for the chip keybindings / click handlers.
pub fn toggle_in_set<T: std::hash::Hash + Eq + Clone>(set: &mut HashSet<T>, value: T) {
    if set.contains(&value) {
        set.remove(&value);
    } else {
        set.insert(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ApiRequestSide, ApiResponsePayload, ApiResponseSide};
    use serde_json::Value;

    fn sample(url: &str, method: &str, status: u16) -> Request {
        let exchange = ApiResponsePayload {
            duration: Some(10.0),
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

    #[test]
    fn empty_filter_matches_everything() {
        let f = FilterState::default();
        assert!(f.is_empty());
        assert!(f.matches(&sample("https://x/a", "GET", 200)));
        assert!(f.matches(&sample("https://y/b", "POST", 404)));
    }

    #[test]
    fn url_substring_is_case_insensitive() {
        let f = FilterState {
            url_substring: "LOGIN".to_string(),
            ..Default::default()
        };
        assert!(f.matches(&sample("https://api/auth/login", "POST", 200)));
        assert!(!f.matches(&sample("https://api/users", "GET", 200)));
    }

    #[test]
    fn method_filter_uses_set_membership() {
        let mut methods = HashSet::new();
        methods.insert("POST".to_string());
        let f = FilterState {
            methods,
            ..Default::default()
        };
        assert!(f.matches(&sample("https://x", "POST", 200)));
        assert!(!f.matches(&sample("https://x", "GET", 200)));
    }

    #[test]
    fn status_class_filter_matches_by_class() {
        let mut status_classes = HashSet::new();
        status_classes.insert(StatusClass::ClientError);
        let f = FilterState {
            status_classes,
            ..Default::default()
        };
        assert!(f.matches(&sample("https://x", "GET", 404)));
        assert!(!f.matches(&sample("https://x", "GET", 200)));
    }

    #[test]
    fn filters_compose_with_and() {
        let mut methods = HashSet::new();
        methods.insert("POST".to_string());
        let mut status_classes = HashSet::new();
        status_classes.insert(StatusClass::Success);
        let f = FilterState {
            methods,
            status_classes,
            url_substring: "login".to_string(),
        };
        assert!(f.matches(&sample("https://api/login", "POST", 201)));
        assert!(!f.matches(&sample("https://api/login", "POST", 404)));
        assert!(!f.matches(&sample("https://api/login", "GET", 200)));
        assert!(!f.matches(&sample("https://api/users", "POST", 200)));
    }

    #[test]
    fn toggle_in_set_flips_membership() {
        let mut s: HashSet<String> = HashSet::new();
        toggle_in_set(&mut s, "POST".to_string());
        assert!(s.contains("POST"));
        toggle_in_set(&mut s, "POST".to_string());
        assert!(!s.contains("POST"));
    }

    #[test]
    fn status_class_from_status_handles_classes_and_unknowns() {
        assert_eq!(StatusClass::from_status(200), Some(StatusClass::Success));
        assert_eq!(StatusClass::from_status(301), Some(StatusClass::Redirect));
        assert_eq!(
            StatusClass::from_status(404),
            Some(StatusClass::ClientError)
        );
        assert_eq!(
            StatusClass::from_status(503),
            Some(StatusClass::ServerError)
        );
        assert_eq!(StatusClass::from_status(100), None);
    }
}
