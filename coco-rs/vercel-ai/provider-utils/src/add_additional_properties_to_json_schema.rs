//! Recursively add `additionalProperties: false` to a JSON schema.
//!
//! Required by some providers (e.g. OpenAI strict-mode) that reject schemas
//! with implicit `additionalProperties: true`.

use serde_json::Value;
use serde_json::json;

/// Walk the schema in place, setting `additionalProperties: false` on every
/// object node, recursing into `properties`, `items`, `anyOf`, `allOf`,
/// `oneOf`, and `definitions`.
pub fn add_additional_properties_to_json_schema(schema: &mut Value) {
    let Some(map) = schema.as_object_mut() else {
        return;
    };

    let is_object = match map.get("type") {
        Some(Value::String(s)) if s == "object" => true,
        Some(Value::Array(arr)) => arr.iter().any(|t| t == "object"),
        _ => false,
    };

    if is_object {
        map.insert("additionalProperties".into(), json!(false));
        if let Some(Value::Object(props)) = map.get_mut("properties") {
            for (_, v) in props.iter_mut() {
                visit(v);
            }
        }
    }

    if let Some(items) = map.get_mut("items") {
        match items {
            Value::Array(arr) => {
                for item in arr.iter_mut() {
                    visit(item);
                }
            }
            other => visit(other),
        }
    }

    for key in ["anyOf", "allOf", "oneOf"] {
        if let Some(Value::Array(arr)) = map.get_mut(key) {
            for item in arr.iter_mut() {
                visit(item);
            }
        }
    }

    if let Some(Value::Object(defs)) = map.get_mut("definitions") {
        for (_, v) in defs.iter_mut() {
            visit(v);
        }
    }
}

fn visit(def: &mut Value) {
    if def.is_boolean() {
        return;
    }
    add_additional_properties_to_json_schema(def);
}

#[cfg(test)]
#[path = "add_additional_properties_to_json_schema.test.rs"]
mod tests;
