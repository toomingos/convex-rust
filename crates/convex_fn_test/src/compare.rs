//! Output comparison modes from bench/cases.json (`exact`, `shape`, `approx`,
//! `ignore`). Values are parsed JSON, so key order and number formatting
//! (`3` vs `3.0`) never matter.

use serde_json::Value;

/// `Ok(())` if `a` matches `b`, else `Err(json_path_of_first_difference)`.
pub fn matches(mode: &str, tolerance: f64, a: &Value, b: &Value) -> Result<(), String> {
    match mode {
        "ignore" => {
            // Only success vs. error must agree.
            if a.get("ok").is_some() == b.get("ok").is_some() {
                Ok(())
            } else {
                Err("$ (ok vs error)".into())
            }
        }
        "exact" => walk(a, b, "$", &|x, y| x == y),
        "approx" => walk(a, b, "$", &|x, y| {
            x == y || (x - y).abs() <= tolerance * x.abs().max(y.abs()).max(f64::MIN_POSITIVE)
        }),
        "shape" => shape(a, b, "$"),
        other => Err(format!("unknown compare mode {other}")),
    }
}

fn walk(a: &Value, b: &Value, path: &str, num_eq: &dyn Fn(f64, f64) -> bool) -> Result<(), String> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (
                x.as_f64().unwrap_or(f64::NAN),
                y.as_f64().unwrap_or(f64::NAN),
            );
            if num_eq(x, y) {
                Ok(())
            } else {
                Err(path.into())
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                return Err(format!("{path}.length ({} vs {})", x.len(), y.len()));
            }
            x.iter()
                .zip(y)
                .enumerate()
                .try_for_each(|(i, (x, y))| walk(x, y, &format!("{path}[{i}]"), num_eq))
        }
        (Value::Object(x), Value::Object(y)) => {
            if x.len() != y.len() || x.keys().any(|k| !y.contains_key(k)) {
                return Err(format!("{path} (keys differ)"));
            }
            x.iter()
                .try_for_each(|(k, v)| walk(v, &y[k], &format!("{path}.{k}"), num_eq))
        }
        _ if a == b => Ok(()),
        _ => Err(path.into()),
    }
}

/// Same structure and JSON types; leaf values may differ. Arrays must have the
/// same length; Convex tagged values (`$integer`, `$bytes`, `$float`) count as types.
fn shape(a: &Value, b: &Value, path: &str) -> Result<(), String> {
    match (a, b) {
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                return Err(format!("{path}.length ({} vs {})", x.len(), y.len()));
            }
            x.iter()
                .zip(y)
                .enumerate()
                .try_for_each(|(i, (x, y))| shape(x, y, &format!("{path}[{i}]")))
        }
        (Value::Object(x), Value::Object(y)) => {
            if x.len() != y.len() || x.keys().any(|k| !y.contains_key(k)) {
                return Err(format!("{path} (keys differ)"));
            }
            x.iter()
                .try_for_each(|(k, v)| shape(v, &y[k], &format!("{path}.{k}")))
        }
        (Value::Null, Value::Null)
        | (Value::Bool(_), Value::Bool(_))
        | (Value::Number(_), Value::Number(_))
        | (Value::String(_), Value::String(_)) => Ok(()),
        _ => Err(format!("{path} (type differs)")),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn modes() {
        assert!(matches("exact", 0.0, &json!({"ok": 3.0}), &json!({"ok": 3})).is_ok());
        assert!(matches("exact", 0.0, &json!({"ok": [1, 2]}), &json!({"ok": [1, 3]})).is_err());
        assert!(matches(
            "approx",
            1e-9,
            &json!({"ok": 1.0}),
            &json!({"ok": 1.0 + 1e-12})
        )
        .is_ok());
        assert!(matches(
            "shape",
            0.0,
            &json!({"ok": {"a": "x"}}),
            &json!({"ok": {"a": "y"}})
        )
        .is_ok());
        assert!(matches(
            "shape",
            0.0,
            &json!({"ok": {"a": "x"}}),
            &json!({"ok": {"a": 1}})
        )
        .is_err());
        assert!(matches("ignore", 0.0, &json!({"ok": 1}), &json!({"error": {}})).is_err());
    }
}
