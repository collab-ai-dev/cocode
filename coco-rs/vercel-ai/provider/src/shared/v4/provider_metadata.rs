//! Provider metadata type.

use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;

use crate::json_value::JSONValue;

/// Provider-specific metadata attached to responses or stream events.
///
/// Similar to ProviderOptions but used for data returned from providers.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderMetadata(pub HashMap<String, JSONValue>);

impl ProviderMetadata {
    /// Create a new empty ProviderMetadata.
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    /// Create ProviderMetadata from a HashMap.
    pub fn from_map(map: HashMap<String, JSONValue>) -> Self {
        Self(map)
    }

    /// Get a metadata value by key.
    pub fn get(&self, key: &str) -> Option<&JSONValue> {
        self.0.get(key)
    }

    /// Set a metadata value.
    pub fn set(&mut self, key: impl Into<String>, value: JSONValue) {
        self.0.insert(key.into(), value);
    }

    /// Check if there is any metadata.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
#[path = "provider_metadata.test.rs"]
mod tests;
