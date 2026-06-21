//! Removes JSON Schema keywords that Anthropic rejects in
//! `output_config.format.schema`.
//!
//! The full original schema is still used by AI SDK result validation. This
//! only relaxes the schema sent to Anthropic's constrained decoder.

use serde_json::Map;
use serde_json::Value;

const SUPPORTED_STRING_FORMATS: &[&str] = &[
    "date-time",
    "time",
    "date",
    "duration",
    "email",
    "hostname",
    "uri",
    "ipv4",
    "ipv6",
    "uuid",
];

const DESCRIPTION_CONSTRAINT_KEYS: &[&str] = &[
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "multipleOf",
    "minLength",
    "maxLength",
    "pattern",
    "minItems",
    "maxItems",
    "uniqueItems",
    "minProperties",
    "maxProperties",
    "not",
];

/// Remove JSON Schema keywords that Anthropic rejects in structured-output schemas.
pub fn sanitize_json_schema(schema: &Value) -> Value {
    sanitize_schema(schema)
}

fn sanitize_definition(def: &Value) -> Value {
    match def {
        Value::Bool(_) | Value::Null => def.clone(),
        Value::Object(_) => sanitize_schema(def),
        _ => def.clone(),
    }
}

fn sanitize_schema(schema: &Value) -> Value {
    let obj = match schema.as_object() {
        Some(o) => o,
        None => return schema.clone(),
    };

    // $ref short-circuits — Anthropic only needs the reference.
    if obj.contains_key("$ref") {
        let mut out = Map::new();
        out.insert("$ref".into(), obj["$ref"].clone());
        return Value::Object(out);
    }

    let mut result = Map::new();

    for key in &[
        "$schema",
        "$id",
        "title",
        "description",
        "default",
        "const",
        "enum",
        "type",
    ] {
        if let Some(v) = obj.get(*key) {
            result.insert((*key).into(), v.clone());
        }
    }

    // anyOf / oneOf → anyOf
    if let Some(any_of) = obj.get("anyOf") {
        if let Some(arr) = any_of.as_array() {
            result.insert(
                "anyOf".into(),
                arr.iter().map(sanitize_definition).collect(),
            );
        }
    } else if let Some(one_of) = obj.get("oneOf")
        && let Some(arr) = one_of.as_array()
    {
        result.insert(
            "anyOf".into(),
            arr.iter().map(sanitize_definition).collect(),
        );
    }

    if let Some(all_of) = obj.get("allOf")
        && let Some(arr) = all_of.as_array()
    {
        result.insert(
            "allOf".into(),
            arr.iter().map(sanitize_definition).collect(),
        );
    }

    // definitions / $defs
    for key in &["definitions", "$defs"] {
        if let Some(Value::Object(defs)) = obj.get(*key) {
            let sanitized: Map<String, Value> = defs
                .iter()
                .map(|(k, v)| (k.clone(), sanitize_definition(v)))
                .collect();
            result.insert((*key).into(), Value::Object(sanitized));
        }
    }

    // Object properties
    if obj.get("type").and_then(Value::as_str) == Some("object") || obj.contains_key("properties") {
        if let Some(Value::Object(props)) = obj.get("properties") {
            let sanitized: Map<String, Value> = props
                .iter()
                .map(|(k, v)| (k.clone(), sanitize_definition(v)))
                .collect();
            result.insert("properties".into(), Value::Object(sanitized));
        }
        result.insert("additionalProperties".into(), Value::Bool(false));
        if let Some(req) = obj.get("required") {
            result.insert("required".into(), req.clone());
        }
    }

    // Array items
    if let Some(items) = obj.get("items") {
        let sanitized = match items {
            Value::Array(arr) => arr.iter().map(sanitize_definition).collect(),
            _ => sanitize_definition(items),
        };
        result.insert("items".into(), sanitized);
    }

    // String format (allowlist)
    if let Some(Value::String(fmt)) = obj.get("format")
        && SUPPORTED_STRING_FORMATS.contains(&fmt.as_str())
    {
        result.insert("format".into(), Value::String(fmt.clone()));
    }

    // Constraint descriptions
    if let Some(constraint_desc) = get_constraint_description(obj) {
        let key = String::from("description");
        let entry = result.entry(key).or_insert(Value::Null);
        *entry = if entry.is_null() {
            Value::String(constraint_desc)
        } else if let Value::String(existing) = entry {
            Value::String(format!("{existing}\n{constraint_desc}"))
        } else {
            Value::String(constraint_desc)
        };
    }

    Value::Object(result)
}

fn get_constraint_description(obj: &Map<String, Value>) -> Option<String> {
    let mut parts: Vec<String> = DESCRIPTION_CONSTRAINT_KEYS
        .iter()
        .filter_map(|key| {
            let v = obj.get(*key)?;
            if v.is_null() || v == &Value::Bool(false) {
                return None;
            }
            Some(format!(
                "{}: {}",
                format_constraint_name(key),
                format_constraint_value(v)
            ))
        })
        .collect();

    // unsupported format
    if let Some(Value::String(fmt)) = obj.get("format")
        && !SUPPORTED_STRING_FORMATS.contains(&fmt.as_str())
    {
        parts.push(format!("format: {fmt}"));
    }

    if parts.is_empty() {
        None
    } else {
        Some(format!("{}.", parts.join("; ")))
    }
}

fn format_constraint_name(key: &str) -> String {
    let mut out = String::with_capacity(key.len() + 4);
    for ch in key.chars() {
        if ch.is_uppercase() {
            out.push(' ');
            out.push(ch.to_lowercase().next().unwrap_or(ch));
        } else {
            out.push(ch);
        }
    }
    out
}

fn format_constraint_value(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        _ => v.to_string(),
    }
}

#[cfg(test)]
#[path = "sanitize_json_schema.test.rs"]
mod tests;
