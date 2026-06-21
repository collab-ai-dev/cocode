//! Language model V4 tool type.
//!
//! Union type for tools that can be passed to a language model.

use serde::Deserialize;
use serde::Serialize;

use super::function_tool::LanguageModelV4FunctionTool;
use super::provider_tool::LanguageModelV4ProviderTool;

/// A tool that can be passed to a language model.
///
/// This is the union type for all tool kinds supported by the V4 API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum LanguageModelV4Tool {
    /// A function tool with a name, description, and input schema.
    Function(LanguageModelV4FunctionTool),
    /// A provider-defined tool (e.g., MCP tools).
    Provider(LanguageModelV4ProviderTool),
}

impl LanguageModelV4Tool {
    /// Create a function tool.
    pub fn function(tool: LanguageModelV4FunctionTool) -> Self {
        Self::Function(tool)
    }

    /// Create a provider tool.
    pub fn provider(tool: LanguageModelV4ProviderTool) -> Self {
        Self::Provider(tool)
    }

    /// Get the tool name.
    pub fn name(&self) -> &str {
        match self {
            Self::Function(t) => &t.name,
            Self::Provider(t) => &t.name,
        }
    }

    /// Check if this is a function tool.
    pub fn is_function(&self) -> bool {
        matches!(self, Self::Function(_))
    }

    /// Get the inner function tool if this is a function tool.
    pub fn as_function(&self) -> Option<&LanguageModelV4FunctionTool> {
        match self {
            Self::Function(t) => Some(t),
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "tool.test.rs"]
mod tests;
