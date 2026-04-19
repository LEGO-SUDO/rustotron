//! Undo Reactotron's falsy-value sentinel mangling.
//!
//! `reactotron-core-client/src/serialize.ts` replaces certain JS values
//! with string sentinels before `JSON.stringify`, because the author
//! wanted `undefined`, `null`, `false`, `0`, `""` to survive a JSON
//! round-trip distinguishably. The upstream Reactotron **server**
//! (`repair-serialization.ts`) undoes the mapping on ingest before
//! fanning messages out. Rustotron v1 originally shipped without that
//! repair pass on the theory that sentinels wouldn't hit our typed
//! surface (headers / status / duration are normally non-falsy).
//!
//! In practice, the RN networking plugin emits `"~~~ null ~~~"` for
//! `response.headers` when a fetch completes with no headers exposed
//! (e.g. CORS-blocked responses). That string doesn't satisfy our
//! `Option<HashMap<String, String>>` schema, the whole frame falls
//! through to `Message::Unknown`, and the user loses the row.
//!
//! This module runs a recursive rewrite over the parsed JSON
//! **before** strict deserialisation:
//!
//! | Sentinel string             | Replaced with     |
//! |-----------------------------|-------------------|
//! | `"~~~ null ~~~"`            | `null`            |
//! | `"~~~ undefined ~~~"`       | `null`            |
//! | `"~~~ false ~~~"`           | `false`           |
//! | `"~~~ zero ~~~"`            | `0`               |
//! | `"~~~ empty string ~~~"`    | `""`              |
//! | `"~~~ NaN ~~~"`             | `null` (JSON-safe)|
//!
//! Kept as strings (they're semantic placeholders, not data we want to
//! reinterpret): `"~~~ skipped ~~~"`, `"~~~ Circular Reference ~~~"`,
//! `"~~~ Infinity ~~~"`, `"~~~ -Infinity ~~~"`, `"~~~ <fn> () ~~~"`.
//!
//! The repair walks every map value and array element recursively.
//! String keys are not rewritten — Reactotron never encodes falsy
//! values as JSON keys.

use serde_json::{Map, Value};

/// Rewrite `value` in-place, replacing Reactotron falsy-value sentinels
/// with their JSON-native equivalents. Strings that are not sentinels
/// are untouched.
pub fn repair(value: &mut Value) {
    match value {
        Value::String(s) => {
            if let Some(replacement) = sentinel_replacement(s) {
                *value = replacement;
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                repair(item);
            }
        }
        Value::Object(map) => {
            for (_, v) in map.iter_mut() {
                repair(v);
            }
        }
        _ => {}
    }
}

/// Same as [`repair`] but operates on an object (the common entrypoint
/// for payload repair).
pub fn repair_map(map: &mut Map<String, Value>) {
    for (_, v) in map.iter_mut() {
        repair(v);
    }
}

fn sentinel_replacement(s: &str) -> Option<Value> {
    match s {
        "~~~ null ~~~" | "~~~ undefined ~~~" | "~~~ NaN ~~~" => Some(Value::Null),
        "~~~ false ~~~" => Some(Value::Bool(false)),
        "~~~ zero ~~~" => Some(Value::Number(0.into())),
        "~~~ empty string ~~~" => Some(Value::String(String::new())),
        // Kept as strings — see module docs.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn replaces_null_sentinel_at_top_level() {
        let mut v = Value::String("~~~ null ~~~".to_string());
        repair(&mut v);
        assert_eq!(v, Value::Null);
    }

    #[test]
    fn replaces_sentinels_in_nested_object_and_array() {
        let mut v = json!({
            "request": {
                "headers": "~~~ null ~~~",
                "method": "GET",
                "params": "~~~ undefined ~~~",
            },
            "response": {
                "status": 200,
                "headers": "~~~ null ~~~",
                "body": ["~~~ null ~~~", "~~~ false ~~~", 1, "real"],
            },
        });
        repair(&mut v);
        assert_eq!(v["request"]["headers"], Value::Null);
        assert_eq!(v["request"]["method"], "GET");
        assert_eq!(v["request"]["params"], Value::Null);
        assert_eq!(v["response"]["headers"], Value::Null);
        assert_eq!(v["response"]["body"][0], Value::Null);
        assert_eq!(v["response"]["body"][1], Value::Bool(false));
        assert_eq!(v["response"]["body"][2], json!(1));
        assert_eq!(v["response"]["body"][3], "real");
    }

    #[test]
    fn leaves_semantic_placeholders_intact() {
        let mut v = json!({
            "body": "~~~ skipped ~~~",
            "loop": "~~~ Circular Reference ~~~",
            "inf": "~~~ Infinity ~~~",
        });
        repair(&mut v);
        assert_eq!(v["body"], "~~~ skipped ~~~");
        assert_eq!(v["loop"], "~~~ Circular Reference ~~~");
        assert_eq!(v["inf"], "~~~ Infinity ~~~");
    }

    #[test]
    fn replaces_zero_sentinel_with_numeric_zero() {
        let mut v = json!({"deltaTime": "~~~ zero ~~~"});
        repair(&mut v);
        assert_eq!(v["deltaTime"], json!(0));
    }

    #[test]
    fn replaces_empty_string_sentinel() {
        let mut v = json!({"name": "~~~ empty string ~~~"});
        repair(&mut v);
        assert_eq!(v["name"], "");
    }

    #[test]
    fn repair_is_idempotent() {
        let mut v = json!({"headers": "~~~ null ~~~"});
        repair(&mut v);
        let once = v.clone();
        repair(&mut v);
        assert_eq!(v, once);
    }
}
