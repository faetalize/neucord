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

#[cfg(test)]
mod tests {
    #[test]
    fn describes_structure_without_values() {
        let v = serde_json::json!({"id": "123", "secret": "hunter2", "list": [{"n": 1}, {"n": 2}], "x": null});
        let shape = super::describe(&v);
        assert_eq!(shape, "{id: str, list: [{n: num}; 2], secret: str, x: null}");
        assert!(!shape.contains("hunter2"));
    }
}
