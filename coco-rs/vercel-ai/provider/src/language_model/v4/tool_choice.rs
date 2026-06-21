//! Language model V4 tool choice type.
//!
//! Configuration for how the model should select tools.

use serde::Deserialize;
use serde::Serialize;

/// Tool choice configuration.
///
/// Controls how the model selects tools during generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum LanguageModelV4ToolChoice {
    /// The tool selection is automatic (can be no tool).
    Auto,
    /// No tool must be selected.
    None,
    /// One of the available tools must be selected.
    Required,
    /// A specific tool must be selected.
    Tool {
        /// The name of the tool to use.
        #[serde(rename = "toolName")]
        tool_name: String,
    },
}

impl LanguageModelV4ToolChoice {
    /// Create an "auto" tool choice.
    pub fn auto() -> Self {
        Self::Auto
    }

    /// Create a "none" tool choice.
    pub fn none() -> Self {
        Self::None
    }

    /// Create a "required" tool choice.
    pub fn required() -> Self {
        Self::Required
    }

    /// Create a specific tool choice.
    pub fn tool(name: impl Into<String>) -> Self {
        Self::Tool {
            tool_name: name.into(),
        }
    }
}

impl Default for LanguageModelV4ToolChoice {
    fn default() -> Self {
        Self::auto()
    }
}

#[cfg(test)]
#[path = "tool_choice.test.rs"]
mod tests;
