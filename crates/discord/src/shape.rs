//! Debug aid: describe the *structure* of a payload (field names and value
//! types) without any values, so parsers can be checked against what Discord
//! actually sends to Neucord. Enable with `NEUCORD_DEBUG_SHAPES=1`.

use serde_json::Value;

pub fn enabled() -> bool {
    std::env::var_os("NEUCORD_DEBUG_SHAPES").is_some_and(|v| v != "0")
}

/// `{id: str, channels: [{id: str, type: num}; 12], icon: null}`
pub fn describe(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(_) => "bool".into(),
        Value::Number(_) => "num".into(),
        Value::String(_) => "str".into(),
        Value::Array(items) => match items.first() {
            None => "[]".into(),
            Some(first) => format!("[{}; {}]", describe(first), items.len()),
        },
        Value::Object(map) => {
            let fields: Vec<String> = map.iter().map(|(k, v)| format!("{k}: {}", describe(v))).collect();
            format!("{{{}}}", fields.join(", "))
        }
    }
}

/// Like [`describe`], but steadier from one session to the next, for
/// comparing over time: no counts, every field any element of an array has,
/// and maps keyed by ids (`notes`, `assignments`) shown as one `<id>` entry,
/// so no id ends up in it either.
pub fn outline(value: &Value) -> String {
    match value {
        Value::Array(items) if items.iter().all(Value::is_object) && !items.is_empty() => {
            let mut merged = serde_json::Map::new();
            for item in items.iter().filter_map(Value::as_object) {
                for (k, v) in item {
                    if !merged.contains_key(k) || merged[k].is_null() {
                        merged.insert(k.clone(), v.clone());
                    }
                }
            }
            format!("[{}]", outline(&Value::Object(merged)))
        }
        Value::Array(items) => match items.first() {
            None => "[]".into(),
            Some(first) => format!("[{}]", outline(first)),
        },
        Value::Object(map) if !map.is_empty() && map.keys().all(|k| k.bytes().all(|b| b.is_ascii_digit())) => {
            format!("{{<id>: {}}}", outline(map.values().next().unwrap_or(&Value::Null)))
        }
        Value::Object(map) => {
            let fields: Vec<String> = map.iter().map(|(k, v)| format!("{k}: {}", outline(v))).collect();
            format!("{{{}}}", fields.join(", "))
        }
        other => describe(other),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn describes_structure_without_values() {
        let v = serde_json::json!({"id": "123", "secret": "hunter2", "list": [{"n": 1}, {"n": 2}], "x": null});
        let shape = super::describe(&v);
        assert_eq!(shape, "{id: str, list: [{n: num}; 2], secret: str, x: null}");
        assert!(!shape.contains("hunter2"));
    }

    #[test]
    fn outlines_merge_elements_and_hide_ids() {
        let v = serde_json::json!({
            "guilds": [{"id": "1"}, {"id": "2", "lazy": true}],
            "notes": {"123": "a", "456": "b"},
            "list": [1, 2, 3],
        });
        assert_eq!(super::outline(&v), "{guilds: [{id: str, lazy: bool}], list: [num], notes: {<id>: str}}");
    }
}
