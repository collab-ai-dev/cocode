//! Self-validating tool input schema: the [`ToolInputSchema`] newtype owns
//! both the JSON Schema document (model-facing serialization via
//! [`ToolInputSchema::as_value`]) AND the compiled [`jsonschema::Validator`]
//! (runtime validation via [`ToolInputSchema::validate`]), built ONCE at
//! construction.
//!
//! Validates model input against the tool's full schema — both on
//! pre-execution validation AND on re-validation after a PreToolUse hook
//! rewrites the input (`tool_call_preparer.rs`). [`ToolInputSchema::validate`]
//! is sync, lock-free, and classification-identical.
//!
//! ## Construction
//!
//! - [`ToolInputSchema::from_input_type`] — Bucket-A tools: derive from a
//!   `T: JsonSchema` input struct, close it (`additionalProperties:false`),
//!   then compile. A failure is a tool-author bug ⇒ panic with the type name
//!   (the registry force-init gate catches it before ship).
//! - [`ToolInputSchema::from_value`] — hand-built / MCP-wire / `--json-schema`
//!   tools: normalize the root (fold in `type:"object"` when absent; reject an
//!   explicit non-object root), then compile (= meta-validation) and KEEP the
//!   validator.
//!
//! ## No cache
//!
//! The validator lives inside the schema — compiled once, shared behind `Arc`
//! on clone. There is no separate `ToolId`-keyed cache, no lazy compile, and no
//! MCP-reconnect staleness: a reconnect builds a new tool ⇒ new schema ⇒ new
//! validator, and the registry overwrite drops the old one.
//!
//! ## Build invariant
//!
//! `jsonschema` stays `default-features = false` (resolve-http OFF) so a remote
//! `$ref` is rejected as a graceful `Err` at construction — never fetched
//! (SSRF / blocking-fetch guard for untrusted MCP schemas).
//!
//! ## Error shape
//!
//! Construction failures produce [`SchemaError`] (`StatusCode::InvalidArguments`);
//! validation failures produce `Vec<`[`SchemaIssue`]`>` carrying compact,
//! model-facing messages derived from `jsonschema::ValidationError`.

use std::sync::Arc;

use serde_json::Map;
use serde_json::Value;

use crate::derive::derive_input_schema_value;
use coco_error::ErrorExt;
use coco_error::StackError;
use coco_error::StatusCode;
use schemars::JsonSchema;

/// Self-validating tool input schema (v4.2 owner — supersedes the deleted
/// `coco_types::ToolInputSchema`). Owns the JSON Schema document (for
/// model-facing serialization via [`Self::as_value`]) AND the compiled
/// validator (for runtime validation via [`Self::validate`]), compiled ONCE at
/// construction. Cheap to clone — the validator is shared behind `Arc`.
///
/// This collapses the old separate `ToolSchemaValidator` cache: there is no
/// lazy compile, no `ToolId`-keyed lookup, and no MCP-reconnect staleness (a
/// reconnect builds a new tool → new schema → new validator; the registry
/// overwrite drops the old one).
#[derive(Clone)]
pub struct ToolInputSchema {
    value: Value,
    validator: Arc<jsonschema::Validator>,
}

impl std::fmt::Debug for ToolInputSchema {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The compiled validator tree is large and noisy; elide it.
        f.debug_struct("ToolInputSchema")
            .field("value", &self.value)
            .field("validator", &"<compiled>")
            .finish()
    }
}

impl ToolInputSchema {
    /// Bucket A entry — schemars-derived from `T`, closed with
    /// `additionalProperties: false`, then compiled. A failure here is a
    /// tool-author bug, so it panics with the offending type name (the
    /// coco-tools force-init test turns that into a CI failure, not a
    /// production-only panic).
    #[must_use]
    pub fn from_input_type<T: JsonSchema>() -> Self {
        let mut raw = derive_input_schema_value::<T>();
        if let Some(obj) = raw.as_object_mut() {
            obj.insert("additionalProperties".to_string(), Value::Bool(false));
        }
        Self::from_value(raw).unwrap_or_else(|e| {
            panic!(
                "schemars-derived schema for {} failed validation: {e}",
                std::any::type_name::<T>(),
            )
        })
    }

    /// Bucket B/C/D/E entry — programmer-written Value, derive+mutate Value,
    /// external MCP wire schema, or user `--json-schema`. Normalizes the root
    /// (the ONLY mutation: fold-in `type:"object"` when neither `type` nor a
    /// composition keyword is present — see [`normalize_root_object`]), then
    /// compiles the validator (= meta-validation) and keeps it. External
    /// schemas are otherwise preserved verbatim; a remote `$ref` surfaces here
    /// as [`SchemaError::InvalidSchema`] (jsonschema returns `Err`, never
    /// fetches, with `resolve-http` off).
    pub fn from_value(mut raw: Value) -> Result<Self, SchemaError> {
        normalize_root_object(&mut raw)?;
        let validator =
            jsonschema::validator_for(&raw).map_err(|e| SchemaError::InvalidSchema {
                message: e.to_string(),
            })?;
        Ok(Self {
            value: raw,
            validator: Arc::new(validator),
        })
    }

    /// Bucket B/C/E hand-built static schema — an author-written `json!({ … })`
    /// guaranteed to be a valid, closed object schema. Like
    /// [`Self::from_input_type`], a construction failure is a tool-author bug, so
    /// it panics with the meta-validation error (the coco-tools force-init test
    /// turns that into a CI failure, never a production panic). Centralizes the
    /// panic policy so hand-built tools don't each `from_value(..).expect(..)`.
    #[must_use]
    pub fn from_static_value(raw: Value) -> Self {
        Self::from_value(raw).unwrap_or_else(|e| panic!("static tool input schema is invalid: {e}"))
    }

    /// The JSON Schema document (root `type:"object"`). Serialized into the
    /// model-facing tool definition; also the base for
    /// [`schema_omit_properties`].
    #[must_use]
    pub fn as_value(&self) -> &Value {
        &self.value
    }

    /// Validate `input` against the compiled validator. Synchronous and
    /// lock-free — the validator lives in `self`. Returns the same classified
    /// [`SchemaIssue`] list the legacy `validate_collect` produced, so
    /// `app/query::format_schema_error` is unaffected.
    pub fn validate(&self, input: &Value) -> Result<(), Vec<SchemaIssue>> {
        let issues: Vec<SchemaIssue> = self
            .validator
            .iter_errors(input)
            .flat_map(SchemaIssue::from_jsonschema)
            .collect();
        if issues.is_empty() {
            Ok(())
        } else {
            Err(issues)
        }
    }
}

/// Fold-in `type:"object"` when the root omits `type` AND carries no
/// composition keyword (`$ref`/`allOf`/`anyOf`/`oneOf`/`not`); reject an
/// explicit non-object root. This is the ONLY mutation [`ToolInputSchema::from_value`]
/// makes to an external schema. Rejecting array-form `["object","null"]`
/// prevents a `null` input from passing validation for dynamic `Value` tools.
pub(crate) fn normalize_root_object(value: &mut Value) -> Result<(), SchemaError> {
    let Some(obj) = value.as_object_mut() else {
        return Err(SchemaError::RootTypeNotObject);
    };
    match obj.get("type") {
        Some(Value::String(s)) if s == "object" => Ok(()),
        Some(Value::String(s)) if s == "null" => Err(SchemaError::RootTypeNull),
        Some(_) => Err(SchemaError::RootTypeNotObject),
        None => {
            const COMPOSITION: [&str; 5] = ["$ref", "allOf", "anyOf", "oneOf", "not"];
            if COMPOSITION.iter().any(|k| obj.contains_key(*k)) {
                // Composition root — leave the author's contract untouched.
                Ok(())
            } else {
                obj.insert("type".to_string(), Value::String("object".to_string()));
                Ok(())
            }
        }
    }
}

/// Return a clone of `schema` with every name in `fields` removed from
/// `properties` and `required`; drops `required` if it empties. Plural so a
/// multi-field omit (AgentTool hides `mcp_servers` + `run_in_background`) costs
/// a single clone. The model-facing view is never validated, so no recompile.
#[must_use]
pub fn schema_omit_properties(schema: &Value, fields: &[&str]) -> Value {
    let mut out = schema.clone();
    if let Some(obj) = out.as_object_mut() {
        if let Some(props) = obj.get_mut("properties").and_then(Value::as_object_mut) {
            for f in fields {
                props.remove(*f);
            }
        }
        if let Some(req) = obj.get_mut("required").and_then(Value::as_array_mut) {
            req.retain(|v| !matches!(v.as_str(), Some(s) if fields.contains(&s)));
            if req.is_empty() {
                obj.remove("required");
            }
        }
    }
    out
}

/// Canonicalize the model-facing copy of a tool JSON Schema.
///
/// This intentionally does not feed back into runtime validation. It stabilizes
/// provider prompt-cache keys by normalizing only deterministic wire details:
/// object key order, selected unordered keyword arrays, and a missing plain
/// object root type.
#[must_use]
pub fn canonicalize_model_tool_schema(schema: &Value) -> Value {
    let mut out = canonicalize_schema_value(schema, true);
    if matches!(out, Value::Object(ref obj) if obj.is_empty()) {
        out = serde_json::json!({ "type": "object" });
    }
    out
}

fn canonicalize_schema_value(value: &Value, is_root: bool) -> Value {
    match value {
        Value::Object(obj) => {
            let mut out = Map::new();
            let mut keys: Vec<&String> = obj.keys().collect();
            keys.sort();
            for key in keys {
                let Some(raw) = obj.get(key) else {
                    continue;
                };
                match key.as_str() {
                    "required" => {
                        if let Some(required) = sorted_string_array(raw)
                            && !required.is_empty()
                        {
                            out.insert(key.clone(), Value::Array(required));
                        }
                    }
                    "dependentRequired" => {
                        if let Some(deps) = canonical_dependent_required(raw) {
                            out.insert(key.clone(), deps);
                        }
                    }
                    _ => {
                        out.insert(key.clone(), canonicalize_schema_value(raw, false));
                    }
                }
            }

            if is_root
                && !out.contains_key("type")
                && !["$ref", "allOf", "anyOf", "oneOf", "not"]
                    .iter()
                    .any(|key| out.contains_key(*key))
            {
                out.insert("type".to_string(), Value::String("object".to_string()));
            }

            let mut keys: Vec<String> = out.keys().cloned().collect();
            keys.sort();
            Value::Object(
                keys.into_iter()
                    .filter_map(|key| out.remove(&key).map(|value| (key, value)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| canonicalize_schema_value(item, false))
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn sorted_string_array(value: &Value) -> Option<Vec<Value>> {
    let mut strings: Vec<String> = value
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    strings.sort();
    strings.dedup();
    Some(strings.into_iter().map(Value::String).collect())
}

fn canonical_dependent_required(value: &Value) -> Option<Value> {
    let obj = value.as_object()?;
    let mut out = Map::new();
    let mut keys: Vec<&String> = obj.keys().collect();
    keys.sort();
    for key in keys {
        if let Some(raw) = obj.get(key)
            && let Some(required) = sorted_string_array(raw)
        {
            out.insert(key.clone(), Value::Array(required));
        }
    }
    Some(Value::Object(out))
}

/// Construction-time schema error. Tier-3 (`thiserror` plus a manual
/// [`StackError`] / [`ErrorExt`] impl), mirroring `coco_context::ContextError`.
/// Always classifies as [`StatusCode::InvalidArguments`] (non-retryable).
#[derive(Debug, thiserror::Error)]
pub enum SchemaError {
    #[error(
        "schema root must declare type:\"object\" as a single string \
         (composition/array forms like [\"object\",\"null\"] are rejected)"
    )]
    RootTypeNotObject,

    #[error("schema root is the singleton null type")]
    RootTypeNull,

    #[error("schema failed JSON Schema meta-validation: {message}")]
    InvalidSchema { message: String },
}

impl StackError for SchemaError {
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>) {
        buf.push(format!("{layer}: {self}"));
    }

    fn next(&self) -> Option<&dyn StackError> {
        None
    }
}

impl ErrorExt for SchemaError {
    fn status_code(&self) -> StatusCode {
        StatusCode::InvalidArguments
    }

    fn as_any(&self) -> &(dyn std::any::Any + 'static) {
        self
    }
}

/// Test-double helper: a trivial closed `{"type":"object"}` schema for tool
/// stubs whose `runtime_validation_schema` content is irrelevant (they exercise
/// execution / registry / streaming, not schema validation). Available under
/// `cfg(test)` and the `testing` feature so doubles in dependent crates share it.
#[cfg(any(test, feature = "testing"))]
pub fn test_runtime_schema() -> &'static ToolInputSchema {
    static SCHEMA: std::sync::OnceLock<ToolInputSchema> = std::sync::OnceLock::new();
    SCHEMA
        .get_or_init(|| ToolInputSchema::from_static_value(serde_json::json!({ "type": "object" })))
}

/// Implement [`Tool::runtime_validation_schema`](crate::Tool) for a derive-only
/// (Bucket A) tool via a per-impl `OnceLock` cache of
/// [`ToolInputSchema::from_input_type`]. Keeps the tool a unit struct (no
/// field, no `new()`); the schema compiles on first access (the coco-tools
/// force-init test turns a bad schema into a CI panic).
#[macro_export]
macro_rules! impl_runtime_schema {
    ($input:ty) => {
        fn runtime_validation_schema(&self) -> &$crate::ToolInputSchema {
            static SCHEMA: ::std::sync::OnceLock<$crate::ToolInputSchema> =
                ::std::sync::OnceLock::new();
            SCHEMA.get_or_init($crate::ToolInputSchema::from_input_type::<$input>)
        }
    };
}

/// Structured form of a JSON Schema validation issue, captured at the
/// `core/tool-runtime` boundary so higher layers
/// (`app/query::tool_input_validate::format_schema_error`) can produce
/// the error text without depending on `jsonschema` directly.
///
/// Each variant maps onto a `formatZodValidationError`
/// (`utils/toolErrors.ts:66-130`) output line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaIssue {
    /// Required field is missing from the input. `path` is the
    /// JSON Pointer of the parent object; `field` is the missing key.
    MissingRequired { path: String, field: String },
    /// Input contains a field not in the schema.
    UnexpectedField { path: String, field: String },
    /// Field type does not match the schema.
    TypeMismatch {
        path: String,
        expected: String,
        received: String,
    },
    /// Catch-all for other validation failures (enum, pattern,
    /// min/max length, etc). `message` is the raw `jsonschema`
    /// rendering.
    Other { path: String, message: String },
}

impl SchemaIssue {
    /// Classify a `jsonschema::ValidationError` into one or more
    /// `SchemaIssue`s. `additionalProperties` lumps every unexpected key into
    /// a single `jsonschema` error, so it expands to one `UnexpectedField` per
    /// key; every other kind maps to exactly one issue.
    fn from_jsonschema(err: jsonschema::ValidationError<'_>) -> Vec<SchemaIssue> {
        use jsonschema::error::ValidationErrorKind;

        let path = err.instance_path().to_string();
        let message = err.to_string();
        let instance_type = json_type_name(err.instance());
        match err.kind() {
            ValidationErrorKind::Required { property } => vec![SchemaIssue::MissingRequired {
                path,
                field: property
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| property.to_string()),
            }],
            ValidationErrorKind::AdditionalProperties { unexpected } => {
                // jsonschema reports every unexpected key in one error; expand
                // to one issue per key so the formatter renders them
                // line-by-line.
                if unexpected.is_empty() {
                    vec![SchemaIssue::Other { path, message }]
                } else {
                    unexpected
                        .iter()
                        .map(|field| SchemaIssue::UnexpectedField {
                            path: path.clone(),
                            field: field.clone(),
                        })
                        .collect()
                }
            }
            ValidationErrorKind::Type { kind } => {
                let expected = format_type_kind(kind);
                vec![SchemaIssue::TypeMismatch {
                    path,
                    expected,
                    received: instance_type.into(),
                }]
            }
            _ => vec![SchemaIssue::Other { path, message }],
        }
    }
}

fn format_type_kind(kind: &jsonschema::error::TypeKind) -> String {
    use jsonschema::error::TypeKind;
    match kind {
        TypeKind::Single(t) => t.as_str().to_string(),
        TypeKind::Multiple(set) => {
            let names: Vec<&str> = set.iter().map(jsonschema::JsonType::as_str).collect();
            // `jsonschema` never emits an empty type set; the guard is defensive.
            if names.is_empty() {
                "(no types)".to_string()
            } else {
                names.join(", ")
            }
        }
    }
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Format a slice of [`SchemaIssue`]s into the error body.
///
/// Mirrors the structure of `formatZodValidationError`:
/// the body is `"{tool} failed due to the following {issue|issues}:\n{lines}"`,
/// each line maps onto one of three patterns:
///
/// - `MissingRequired` → `"The required parameter \`{path}\` is missing"`
/// - `UnexpectedField` → `"An unexpected parameter \`{key}\` was provided"`
/// - `TypeMismatch` → `"The parameter \`{path}\` type is expected as \`{expected}\` but provided as \`{received}\`"`
/// - `Other` → falls through to the raw `jsonschema` message,
///   prefixed with the path when present.
///
/// Plural / singular selection: ≥2 lines → `"issues"`, otherwise `"issue"`.
pub fn format_schema_error(tool_name: &str, issues: &[SchemaIssue]) -> String {
    if issues.is_empty() {
        return format!("{tool_name} failed schema validation");
    }

    let mut lines = Vec::with_capacity(issues.len());
    for issue in issues {
        match issue {
            SchemaIssue::MissingRequired { path, field } => {
                let full_path = join_path(path, field);
                lines.push(format!("The required parameter `{full_path}` is missing"));
            }
            SchemaIssue::UnexpectedField { field, .. } => {
                lines.push(format!("An unexpected parameter `{field}` was provided"));
            }
            SchemaIssue::TypeMismatch {
                path,
                expected,
                received,
            } => {
                let p = display_path(path);
                lines.push(format!(
                    "The parameter `{p}` type is expected as `{expected}` but provided as `{received}`"
                ));
            }
            SchemaIssue::Other { path, message } => {
                if path.is_empty() {
                    lines.push(message.clone());
                } else {
                    lines.push(format!("`{}`: {message}", display_path(path)));
                }
            }
        }
    }

    let word = if lines.len() > 1 { "issues" } else { "issue" };
    format!(
        "{tool_name} failed due to the following {word}:\n{}",
        lines.join("\n")
    )
}

/// Stitch a parent path + field name into a single user-readable
/// path. `jsonschema` returns paths as JSON Pointers (`/foo/0/bar`);
/// we convert to dotted+bracket form (`foo[0].bar`).
fn join_path(parent: &str, field: &str) -> String {
    let parent = display_path(parent);
    if parent.is_empty() {
        field.to_string()
    } else {
        format!("{parent}.{field}")
    }
}

/// Convert a `/foo/0/bar`-style JSON Pointer into `foo[0].bar`.
fn display_path(pointer: &str) -> String {
    if pointer.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for segment in pointer.split('/').skip(1) {
        if segment.parse::<usize>().is_ok() {
            out.push('[');
            out.push_str(segment);
            out.push(']');
        } else {
            if !out.is_empty() {
                out.push('.');
            }
            out.push_str(segment);
        }
    }
    out
}

#[cfg(test)]
#[path = "schema.test.rs"]
mod tests;
