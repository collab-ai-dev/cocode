//! Rewrite a JSON Schema into an OpenAI-compatible shape **without changing
//! validation semantics destructively**.
//!
//! Two recursive transforms:
//! - `oneOf` → `anyOf`: OpenAI's function-tool schema validator (and several
//!   OpenAI-compatible backends — GLM / DeepSeek / Groq / xAI) reject `oneOf`.
//!   Anthropic (`sanitize_json_schema`) and Google (`convert_json_schema_to_openapi_schema`)
//!   already perform the same rewrite at their wire boundaries, so this brings
//!   the OpenAI family to parity. `anyOf` is a relaxation (at-least-one vs
//!   exactly-one): the model-facing wire schema becomes slightly more permissive
//!   than coco's runtime validator (which still compiles from the original,
//!   un-rewritten schema), so an input exploiting the relaxation is caught by
//!   runtime validation and re-prompted. That wire/runtime divergence is
//!   intentional and already exists for Anthropic/Google.
//! - safe `allOf` flatten: when every `allOf` branch is a plain object schema
//!   with a **disjoint** property set, the branches' `properties`/`required` are
//!   merged into the parent. Any branch that carries a `$ref`, a composition
//!   keyword, `additionalProperties`, `patternProperties`, a non-object `type`,
//!   or a property name overlapping another branch / the parent is **left
//!   verbatim** — a lossy merge there would silently drop a constraint the
//!   runtime validator still enforces.
//!
//! Strict-free: never sets `strict`, never forces `additionalProperties`, never
//! adds non-declared keys to `required`. Idempotent (re-running is a no-op) and
//! byte-identical on schemas that use neither `oneOf` nor a flattenable `allOf`
//! (e.g. every schemars-derived coco tool schema).

use std::collections::HashSet;

use serde_json::Map;
use serde_json::Value;

/// See the module docs. Returns a transformed clone; the input is untouched.
///
/// Recursion is **keyword-aware** (like the Anthropic / Google converters): it
/// only descends into known subschema positions, so a property literally named
/// `oneOf` — or a composition keyword appearing inside a `default` / `const` /
/// `enum` value — is copied verbatim, never misread as a union.
#[must_use]
pub fn to_openai_compatible_schema(schema: &Value) -> Value {
    let Value::Object(map) = schema else {
        // Bool schema or any non-object node — nothing to rewrite.
        return schema.clone();
    };

    let mut out: Map<String, Value> = Map::new();
    // Fold `oneOf` (and any co-located `anyOf`) into a single `anyOf`, but only
    // when `oneOf` is actually present; an `anyOf`-only schema is left as-is. A
    // schema carrying BOTH at one level is degenerate — the merged `anyOf` is
    // broader than the original `oneOf ∧ anyOf`, but OpenAI cannot express that
    // conjunction and coco's runtime validator still enforces the original
    // schema. (Anthropic's converter instead drops `oneOf`; we keep both branch
    // sets so no constraint is lost.)
    let has_one_of = map.contains_key("oneOf");
    let mut any_of: Vec<Value> = Vec::new();

    for (key, value) in map {
        match key.as_str() {
            // Subschema maps `{ name -> schema }`: recurse into values, keep the
            // names verbatim (a property literally named `oneOf` is NOT a union).
            "properties" | "patternProperties" | "$defs" | "definitions" => {
                out.insert(key.clone(), map_subschemas(value));
            }
            // Subschema arrays.
            "allOf" | "prefixItems" => {
                out.insert(key.clone(), array_subschemas(value));
            }
            // Union keywords.
            "oneOf" | "anyOf" => {
                let transformed = array_subschemas(value);
                if has_one_of {
                    match transformed {
                        Value::Array(items) => any_of.extend(items),
                        // Non-array union (malformed) — keep as a single branch.
                        other => any_of.push(other),
                    }
                } else {
                    out.insert("anyOf".to_string(), transformed);
                }
            }
            // `items` is a single subschema (or a draft-4 tuple array).
            "items" => {
                let v = match value {
                    Value::Array(_) => array_subschemas(value),
                    _ => to_openai_compatible_schema(value),
                };
                out.insert(key.clone(), v);
            }
            // Other single-subschema positions.
            "additionalProperties"
            | "not"
            | "if"
            | "then"
            | "else"
            | "propertyNames"
            | "contains" => {
                out.insert(key.clone(), to_openai_compatible_schema(value));
            }
            // Everything else (`type`, `required`, `enum`, `const`, `default`,
            // `examples`, `description`, numeric/string constraints, …) is data,
            // not a subschema — copy verbatim so composition keywords appearing
            // inside a value are never misread.
            _ => {
                out.insert(key.clone(), value.clone());
            }
        }
    }

    if has_one_of {
        out.insert("anyOf".to_string(), Value::Array(any_of));
    }
    flatten_safe_all_of(out)
}

/// Transform every value of a `{ name -> subschema }` map, keeping names verbatim.
fn map_subschemas(value: &Value) -> Value {
    match value {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| (k.clone(), to_openai_compatible_schema(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Transform every element of an array of subschemas.
fn array_subschemas(value: &Value) -> Value {
    match value {
        Value::Array(items) => {
            Value::Array(items.iter().map(to_openai_compatible_schema).collect())
        }
        other => other.clone(),
    }
}

/// Flatten an `allOf` of plain, disjoint object branches into the parent. Bails
/// to the verbatim object on anything unsafe.
fn flatten_safe_all_of(mut map: Map<String, Value>) -> Value {
    let Some(Value::Array(branches)) = map.get("allOf") else {
        return Value::Object(map);
    };
    if branches.is_empty() || !branches.iter().all(is_flattenable_object_branch) {
        return Value::Object(map);
    }

    // Names already present on the parent — branch names must not collide.
    let mut names: HashSet<String> = map
        .get("properties")
        .and_then(Value::as_object)
        .map(|p| p.keys().cloned().collect())
        .unwrap_or_default();

    let mut add_props: Map<String, Value> = Map::new();
    let mut add_required: Vec<Value> = Vec::new();
    for branch in branches {
        let Some(obj) = branch.as_object() else {
            return Value::Object(map);
        };
        if let Some(Value::Object(props)) = obj.get("properties") {
            for (name, sub) in props {
                if !names.insert(name.clone()) {
                    // Overlapping property — refuse to merge, keep allOf verbatim.
                    return Value::Object(map);
                }
                add_props.insert(name.clone(), sub.clone());
            }
        }
        if let Some(Value::Array(req)) = obj.get("required") {
            add_required.extend(req.iter().cloned());
        }
    }

    map.remove("allOf");
    map.entry("type".to_string())
        .or_insert_with(|| Value::String("object".to_string()));
    if let Value::Object(parent_props) = map
        .entry("properties".to_string())
        .or_insert_with(|| Value::Object(Map::new()))
    {
        for (name, sub) in add_props {
            parent_props.insert(name, sub);
        }
    }
    if !add_required.is_empty()
        && let Value::Array(req) = map
            .entry("required".to_string())
            .or_insert_with(|| Value::Array(Vec::new()))
    {
        for r in add_required {
            if !req.contains(&r) {
                req.push(r);
            }
        }
    }
    Value::Object(map)
}

/// A branch is safe to flatten only if it is a plain object schema: `type`
/// `"object"` or absent, and no keyword beyond the small object allowlist (a
/// `$ref`, composition keyword, `additionalProperties`, `patternProperties`, …
/// all force the verbatim path).
fn is_flattenable_object_branch(branch: &Value) -> bool {
    let Some(map) = branch.as_object() else {
        return false;
    };
    match map.get("type") {
        None => {}
        Some(Value::String(s)) if s == "object" => {}
        _ => return false,
    }
    const SAFE_KEYS: [&str; 5] = ["type", "properties", "required", "title", "description"];
    map.keys().all(|k| SAFE_KEYS.contains(&k.as_str()))
}

#[cfg(test)]
#[path = "openai_compatible_schema.test.rs"]
mod tests;
