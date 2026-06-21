//! Telemetry attribute helpers.
//!
//! Utility functions for assembling telemetry operation names and attributes.

use std::collections::HashMap;

/// Assemble a telemetry operation name from function ID and optional operation ID.
pub fn assemble_operation_name(function_id: &str, operation_id: Option<&str>) -> String {
    match operation_id {
        Some(op_id) if !op_id.is_empty() => format!("{function_id}.{op_id}"),
        _ => function_id.to_string(),
    }
}

/// Get base telemetry attributes for an AI operation.
///
/// Returns a map of standard OpenTelemetry semantic convention attributes.
pub fn get_base_telemetry_attributes(
    model_id: &str,
    provider: &str,
    metadata: Option<&HashMap<String, String>>,
) -> HashMap<String, String> {
    let mut attrs = HashMap::new();
    attrs.insert("ai.model.id".to_string(), model_id.to_string());
    attrs.insert("ai.model.provider".to_string(), provider.to_string());

    if let Some(meta) = metadata {
        for (k, v) in meta {
            attrs.insert(format!("ai.telemetry.metadata.{k}"), v.clone());
        }
    }

    attrs
}

/// Select telemetry attributes based on recording preferences.
///
/// Filters attributes to only include those appropriate for the recording level.
pub fn select_telemetry_attributes(
    attributes: HashMap<String, String>,
    record_inputs: bool,
    record_outputs: bool,
) -> HashMap<String, String> {
    attributes
        .into_iter()
        .filter(|(key, _)| {
            if key.starts_with("ai.prompt") || key.starts_with("ai.input") {
                return record_inputs;
            }
            if key.starts_with("ai.response") || key.starts_with("ai.output") {
                return record_outputs;
            }
            true
        })
        .collect()
}

#[cfg(test)]
#[path = "attributes.test.rs"]
mod tests;
