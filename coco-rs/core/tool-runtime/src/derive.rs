//! Schemars 1.2 → JSON Schema document derive helpers.
//!
//! Each tool declares ONE `#[derive(Deserialize, JsonSchema)] struct
//! XxxInput;`. The schema and the typed `execute` parse share a single
//! artifact, so renaming a struct field is caught at `cargo check`
//! instead of drifting silently. Bucket-A tools turn the struct into a
//! closed runtime schema via the `impl_runtime_schema!` macro
//! ([`crate::schema::ToolInputSchema::from_input_type`]); hand-built
//! tools call [`derive_input_schema_value`] and wrap the result in
//! `from_value(json!({ ... }))`.
//!
//! The Rust mirror uses `T: JsonSchema + Deserialize` — `T` provides the schema
//! via the JsonSchema derive and the parsed type via the Deserialize derive.
//!
//! ## Subschema inlining
//!
//! Schemars' default behaviour is to extract reused types into a
//! `$defs` table and reference them via `$ref: "#/$defs/Foo"`. Some
//! schemas instead expand inline; provider tool APIs (Anthropic,
//! OpenAI, Gemini) all accept `$ref` but their handling differs in
//! edge cases. We set [`SchemaSettings::inline_subschemas`] = `true`
//! so the generated schema is a single flat document.

use schemars::JsonSchema;
use schemars::generate::SchemaSettings;
use serde_json::Value;

/// Derive the entire JSON Schema document from a `T: JsonSchema` input
/// struct (subschemas inlined, no `$ref`). The closed runtime schema is
/// built from this by [`crate::schema::ToolInputSchema::from_input_type`]
/// (which adds `additionalProperties:false` and compiles the validator);
/// hand-built / derive-and-mutate tools call this directly and wrap the
/// result in `from_value(json!({ ... }))`.
#[must_use]
pub fn derive_input_schema_value<T: JsonSchema>() -> Value {
    let generator = SchemaSettings::default()
        .with(|s| {
            s.inline_subschemas = true;
        })
        .for_deserialize()
        .into_generator();
    let schema = generator.into_root_schema_for::<T>();
    // `schemars::Schema` always round-trips through `serde_json::Value`
    // by construction; on the off-chance a future schemars upgrade
    // breaks that, surface as `Value::Null` rather than panicking the
    // tool-listing path — the validator will then reject the empty
    // schema and the model gets a clean error.
    serde_json::to_value(&schema).unwrap_or(Value::Null)
}

#[cfg(test)]
#[path = "derive.test.rs"]
mod tests;
