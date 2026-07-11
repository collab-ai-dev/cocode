use serde_json::Value;

/// Recursively remove `additionalProperties: false` entries from a JSON schema.
///
/// xAI's structured-outputs support rejects `additionalProperties: false`, so
/// tool input schemas are sanitized before being sent. Mirrors
/// `remove-additional-properties.ts`.
/// <https://docs.x.ai/developers/model-capabilities/text/structured-outputs>
pub fn remove_additional_properties_false(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(remove_additional_properties_false)
                .collect(),
        ),
        Value::Object(map) => {
            let mut result = serde_json::Map::with_capacity(map.len());
            for (key, property_value) in map {
                if key == "additionalProperties" && *property_value == Value::Bool(false) {
                    continue;
                }
                result.insert(
                    key.clone(),
                    remove_additional_properties_false(property_value),
                );
            }
            Value::Object(result)
        }
        other => other.clone(),
    }
}

#[cfg(test)]
#[path = "remove_additional_properties.test.rs"]
mod tests;
