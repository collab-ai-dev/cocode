use serde_json::Value;
use serde_json::json;

/// Sets the Anthropic container ID in the provider options based on
/// any previous step's provider metadata.
///
/// Searches backwards through steps to find the most recent container ID.
/// You can use this function in `prepareStep` to forward the container ID between steps.
///
/// Each step should have an optional `providerMetadata` map. The function looks for
/// `providerMetadata["anthropic"]["container"]["id"]` in each step.
///
/// Returns `Some(providerOptions)` with the container ID if found, `None` otherwise.
pub fn forward_anthropic_container_id_from_last_step(
    steps: &[StepMetadata],
) -> Option<ContainerForward> {
    // Search backwards through steps to find the most recent container ID
    for step in steps.iter().rev() {
        if let Some(ref pm) = step.provider_metadata {
            let container_id = pm
                .get("anthropic")
                .and_then(|a| a.get("container"))
                .and_then(|c| c.get("id"))
                .and_then(|id| id.as_str());

            if let Some(id) = container_id {
                return Some(ContainerForward {
                    provider_options: json!({
                        "anthropic": {
                            "container": { "id": id }
                        }
                    }),
                });
            }
        }
    }

    None
}

/// Step metadata containing provider metadata for container ID lookup.
pub struct StepMetadata {
    pub provider_metadata: Option<Value>,
}

/// Result of forwarding a container ID.
pub struct ContainerForward {
    pub provider_options: Value,
}

#[cfg(test)]
#[path = "forward_container_id.test.rs"]
mod tests;
