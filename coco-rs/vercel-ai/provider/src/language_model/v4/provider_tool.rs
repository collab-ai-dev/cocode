//! Provider tool types.
//!
//! Provider tools are tools that are specific to a certain provider.
//! The input and output schemas are defined by the provider, and
//! some of the tools are also executed on the provider systems.

use std::collections::HashMap;

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JSONValue;

/// A tool that is defined and potentially executed by a provider.
///
/// Provider tools have their input and output schemas defined by the provider,
/// and may be executed on the provider's systems (e.g., MCP tools).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageModelV4ProviderTool {
    /// The ID of the tool. Should follow the format `<provider-id>.<unique-tool-name>`.
    pub id: String,
    /// The name of the tool. Unique within this model call.
    pub name: String,
    /// The arguments for configuring the tool.
    /// Must match the expected arguments defined by the provider for this tool.
    #[serde(default)]
    pub args: HashMap<String, JSONValue>,
}

impl LanguageModelV4ProviderTool {
    /// Create a new provider tool.
    pub fn new(provider_id: impl Into<String>, tool_name: impl Into<String>) -> Self {
        let provider_id = provider_id.into();
        let tool_name = tool_name.into();
        Self {
            id: format!("{provider_id}.{tool_name}"),
            name: tool_name,
            args: HashMap::new(),
        }
    }

    /// Create from a full ID (e.g., "provider.tool-name").
    pub fn from_id(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            args: HashMap::new(),
        }
    }

    /// Add an argument for the tool configuration.
    pub fn with_arg(mut self, key: impl Into<String>, value: JSONValue) -> Self {
        self.args.insert(key.into(), value);
        self
    }

    /// Set all arguments.
    pub fn with_args(mut self, args: HashMap<String, JSONValue>) -> Self {
        self.args = args;
        self
    }
}

#[cfg(test)]
#[path = "provider_tool.test.rs"]
mod tests;
