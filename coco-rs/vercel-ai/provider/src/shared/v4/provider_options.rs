//! Provider options type.

use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;

use crate::json_value::JSONValue;

/// Provider-specific options that can be passed to various API calls.
///
/// This is a map of provider names to their specific options.
/// For example: `{ "anthropic": { "thinking": { "type": "enabled" } } }`
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderOptions(pub HashMap<String, HashMap<String, JSONValue>>);

impl ProviderOptions {
    /// Create a new empty ProviderOptions.
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    /// Create ProviderOptions from a HashMap.
    pub fn from_map(map: HashMap<String, HashMap<String, JSONValue>>) -> Self {
        Self(map)
    }

    /// Get options for a specific provider.
    pub fn get(&self, provider: &str) -> Option<&HashMap<String, JSONValue>> {
        self.0.get(provider)
    }

    /// Set options for a specific provider.
    pub fn set(&mut self, provider: impl Into<String>, options: HashMap<String, JSONValue>) {
        self.0.insert(provider.into(), options);
    }

    /// Check if there are any options set.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
#[path = "provider_options.test.rs"]
mod tests;
