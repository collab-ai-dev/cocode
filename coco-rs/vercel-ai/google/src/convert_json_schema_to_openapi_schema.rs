//! Convert JSON Schema 7 to OpenAPI 3.0 schema format.
//!
//! Google's API expects OpenAPI 3.0 schema format, not JSON Schema 7.

use serde_json::Map;
use serde_json::Value;

/// Convert a JSON Schema to OpenAPI 3.0 schema format.
///
/// This performs a recursive conversion, handling:
/// - Empty object schemas (return `None` at root, `{ type: "object" }` nested)
/// - Boolean schemas (`true`/`false`) -> `{ type: "boolean", properties: {} }`
/// - Type arrays -> anyOf
/// - anyOf with null-type flattening
/// - anyOf/oneOf/allOf pass-through with recursive conversion
/// - `const` -> `enum` with single value
/// - Removal of `$schema`, `additionalProperties`, `$id`, `$anchor`, `$comment`
/// - `minLength` preservation
pub fn convert_json_schema_to_openapi_schema(schema: &Value) -> Option<Value> {
    convert_inner(schema, true)
}

/// When every `oneOf` alternative is a singleton enum (`{"enum": [v]}`)
/// sharing the same `type`, return `(shared_type, [v1, v2, ...])`.
/// Otherwise return `None` so the caller falls back to verbatim
/// pass-through. The shared type must be one of the OpenAPI primitive
/// types (`string`/`integer`/`number`/`boolean`) — Gemini-3 requires it.
fn try_coalesce_singleton_enum(schemas: &[Value]) -> Option<(String, Vec<Value>)> {
    if schemas.is_empty() {
        return None;
    }
    let mut shared_type: Option<String> = None;
    let mut enum_values = Vec::with_capacity(schemas.len());
    for s in schemas {
        let obj = s.as_object()?;
        let arr = obj.get("enum")?.as_array()?;
        if arr.len() != 1 {
            return None;
        }
        let ty = obj.get("type").and_then(|v| v.as_str())?;
        match &shared_type {
            None => shared_type = Some(ty.to_string()),
            Some(prev) if prev != ty => return None,
            _ => {}
        }
        enum_values.push(arr[0].clone());
    }
    shared_type.map(|t| (t, enum_values))
}

/// Check if a schema is an empty object schema.
fn is_empty_object_schema(schema: &Value) -> bool {
    let Some(obj) = schema.as_object() else {
        return false;
    };
    if obj.get("type").and_then(|v| v.as_str()) != Some("object") {
        return false;
    }
    let has_empty_or_no_props = match obj.get("properties") {
        None => true,
        Some(Value::Object(props)) => props.is_empty(),
        _ => false,
    };
    // Also check that additionalProperties is not set (matches TS behavior)
    let no_additional_props = matches!(
        obj.get("additionalProperties"),
        None | Some(Value::Bool(false)) | Some(Value::Null)
    );
    has_empty_or_no_props && no_additional_props
}

fn convert_inner(schema: &Value, is_root: bool) -> Option<Value> {
    // Handle boolean schemas (JSON Schema allows true/false as schemas)
    if schema.is_boolean() {
        let mut m = Map::new();
        m.insert("type".to_string(), Value::String("boolean".to_string()));
        m.insert("properties".to_string(), Value::Object(Map::new()));
        return Some(Value::Object(m));
    }

    let obj = schema.as_object()?;

    // Handle empty object schema: undefined at root, { type: "object" } nested
    if is_empty_object_schema(schema) {
        if is_root {
            return None;
        }
        let mut m = Map::new();
        m.insert("type".to_string(), Value::String("object".to_string()));
        if let Some(desc) = obj.get("description").and_then(|v| v.as_str()) {
            m.insert("description".to_string(), Value::String(desc.to_string()));
        }
        return Some(Value::Object(m));
    }

    // Handle truly empty object (no fields at all)
    if obj.is_empty() {
        return Some(Value::Object(Map::new()));
    }

    let mut result = Map::new();

    // Process type field
    if let Some(type_val) = obj.get("type") {
        match type_val {
            Value::Array(types) => {
                // Type array: filter out "null" and emit anyOf only when
                // more than one non-null type remains. Gemini-3 strict
                // mode rejects `anyOf` with siblings (description/format/
                // default/nullable from the pass-through block below), so
                // single-element unions collapse to a plain `type` instead.
                let has_null = types.iter().any(|t| t.as_str() == Some("null"));
                let non_null_types: Vec<&Value> = types
                    .iter()
                    .filter(|t| t.as_str() != Some("null"))
                    .collect();

                match non_null_types.as_slice() {
                    [] => {
                        result.insert("type".to_string(), Value::String("null".to_string()));
                    }
                    [only] => {
                        result.insert("type".to_string(), (*only).clone());
                        if has_null {
                            result.insert("nullable".to_string(), Value::Bool(true));
                        }
                    }
                    many => {
                        let any_of: Vec<Value> = many
                            .iter()
                            .map(|t| {
                                let mut m = Map::new();
                                m.insert("type".to_string(), (*t).clone());
                                Value::Object(m)
                            })
                            .collect();
                        result.insert("anyOf".to_string(), Value::Array(any_of));
                        if has_null {
                            result.insert("nullable".to_string(), Value::Bool(true));
                        }
                    }
                }
            }
            Value::String(_) => {
                result.insert("type".to_string(), type_val.clone());
            }
            _ => {}
        }
    }

    // Convert properties recursively
    if let Some(Value::Object(props)) = obj.get("properties") {
        let mut converted_props = Map::new();
        for (key, value) in props {
            if let Some(converted) = convert_inner(value, false) {
                converted_props.insert(key.clone(), converted);
            }
        }
        if !converted_props.is_empty() {
            result.insert("properties".to_string(), Value::Object(converted_props));
        }
    }

    // Convert items recursively
    if let Some(items) = obj.get("items")
        && let Some(converted) = convert_inner(items, false)
    {
        result.insert("items".to_string(), converted);
    }

    // Handle const -> enum
    if let Some(const_val) = obj.get("const") {
        result.insert("enum".to_string(), Value::Array(vec![const_val.clone()]));
    }

    // Handle anyOf with null-type flattening
    if let Some(Value::Array(schemas)) = obj.get("anyOf") {
        let has_null_type = schemas.iter().any(|s| {
            s.as_object()
                .and_then(|o| o.get("type"))
                .and_then(|t| t.as_str())
                == Some("null")
        });

        if has_null_type {
            let non_null_schemas: Vec<&Value> = schemas
                .iter()
                .filter(|s| {
                    s.as_object()
                        .and_then(|o| o.get("type"))
                        .and_then(|t| t.as_str())
                        != Some("null")
                })
                .collect();

            if non_null_schemas.len() == 1 {
                // Single non-null schema: flatten it into result with nullable
                if let Some(converted) = convert_inner(non_null_schemas[0], false)
                    && let Some(converted_obj) = converted.as_object()
                {
                    result.insert("nullable".to_string(), Value::Bool(true));
                    for (k, v) in converted_obj {
                        result.insert(k.clone(), v.clone());
                    }
                }
            } else {
                // Multiple non-null schemas: keep in anyOf with nullable
                let converted: Vec<Value> = non_null_schemas
                    .iter()
                    .filter_map(|s| convert_inner(s, false))
                    .collect();
                if !converted.is_empty() {
                    result.insert("anyOf".to_string(), Value::Array(converted));
                }
                result.insert("nullable".to_string(), Value::Bool(true));
            }
        } else {
            let converted: Vec<Value> = schemas
                .iter()
                .filter_map(|s| convert_inner(s, false))
                .collect();
            match converted.as_slice() {
                [] => {}
                // Gemini-3 strict mode: `anyOf` may not coexist with
                // sibling keys. A single-element union carries no
                // information, so flatten it into the result (merge the
                // sole alternative's fields). Mirrors the existing
                // null-flatten branch above.
                [only] => {
                    if let Some(only_obj) = only.as_object() {
                        for (k, v) in only_obj {
                            result.insert(k.clone(), v.clone());
                        }
                    }
                }
                _ => {
                    result.insert("anyOf".to_string(), Value::Array(converted));
                }
            }
        }
    }

    // Handle oneOf: Rust `schemars` emits per-variant `oneOf` for
    // string enums (one alternative per variant, each `{const: v,
    // type: "string"}`), to keep per-variant docstrings. Gemini-3
    // rejects this — the outer schema has no `type` field. Coalesce
    // such oneOf into the canonical `{type: "string", enum: [..]}`
    // form (Zod naturally produces this shape upstream; schemars
    // doesn't, hence the divergence). Coalesce runs on the converted
    // alternatives so the `const → enum` rewrite has already happened.
    if let Some(Value::Array(schemas)) = obj.get("oneOf") {
        let converted: Vec<Value> = schemas
            .iter()
            .filter_map(|s| convert_inner(s, false))
            .collect();
        if let Some((shared_type, enum_values)) = try_coalesce_singleton_enum(&converted) {
            result.insert("type".to_string(), Value::String(shared_type));
            result.insert("enum".to_string(), Value::Array(enum_values));
        } else if !converted.is_empty() {
            result.insert("oneOf".to_string(), Value::Array(converted));
        }
    }

    // Handle allOf
    if let Some(Value::Array(schemas)) = obj.get("allOf") {
        let converted: Vec<Value> = schemas
            .iter()
            .filter_map(|s| convert_inner(s, false))
            .collect();
        if !converted.is_empty() {
            result.insert("allOf".to_string(), Value::Array(converted));
        }
    }

    // Pass through selected fields
    for key in &[
        "description",
        "enum",
        "required",
        "minLength",
        "maxLength",
        "minimum",
        "maximum",
        "pattern",
        "format",
        "default",
        "title",
        "minItems",
        "maxItems",
        "nullable",
    ] {
        if let Some(val) = obj.get(*key) {
            // Don't duplicate enum if we already added one from const
            if *key == "enum" && obj.contains_key("const") {
                continue;
            }
            result.insert(key.to_string(), val.clone());
        }
    }

    if is_root {
        // Remove $schema, $id, $anchor, $comment from root
        result.remove("$schema");
        result.remove("$id");
        result.remove("$anchor");
        result.remove("$comment");
    }

    Some(Value::Object(result))
}

#[cfg(test)]
#[path = "convert_json_schema_to_openapi_schema.test.rs"]
mod tests;
