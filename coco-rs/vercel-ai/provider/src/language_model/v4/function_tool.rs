//! Language model V4 function tool type.
//!
//! A tool definition with name, description, and parameters.

use crate::json_schema::JSONSchema;
use crate::json_value::JSONObject;
use crate::shared::ProviderOptions;
use serde::Deserialize;
use serde::Serialize;

/// An input example for a tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolInputExample {
    /// The example input.
    pub input: JSONObject,
}

impl ToolInputExample {
    /// Create a new input example.
    pub fn new(input: JSONObject) -> Self {
        Self { input }
    }
}

/// A function tool definition.
///
/// A tool has a name, a description, and a set of parameters.
///
/// Note: this is **not** the user-facing tool definition. The AI SDK methods will
/// map the user-facing tool definitions to this format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguageModelV4FunctionTool {
    /// The name of the tool. Unique within this model call.
    pub name: String,
    /// A description of the tool. The language model uses this to understand the
    /// tool's purpose and to provide better completion suggestions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The parameters that the tool expects. The language model uses this to
    /// understand the tool's input requirements and to provide matching suggestions.
    pub input_schema: JSONSchema,
    /// An optional list of input examples that show the language
    /// model what the input should look like.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_examples: Option<Vec<ToolInputExample>>,
    /// Strict mode setting for the tool.
    ///
    /// Providers that support strict mode will use this setting to determine
    /// how the input should be generated. Strict mode will always produce
    /// valid inputs, but it might limit what input schemas are supported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
    /// The provider-specific options for the tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
}

impl LanguageModelV4FunctionTool {
    /// Create a new function tool.
    pub fn new(name: impl Into<String>, input_schema: JSONSchema) -> Self {
        Self {
            name: name.into(),
            description: None,
            input_schema,
            input_examples: None,
            strict: None,
            provider_options: None,
        }
    }

    /// Create a function tool with description.
    pub fn with_description(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: JSONSchema,
    ) -> Self {
        Self {
            name: name.into(),
            description: Some(description.into()),
            input_schema,
            input_examples: None,
            strict: None,
            provider_options: None,
        }
    }

    /// Add input examples.
    pub fn with_examples(mut self, examples: Vec<ToolInputExample>) -> Self {
        self.input_examples = Some(examples);
        self
    }

    /// Add a single input example.
    pub fn with_example(mut self, input: JSONObject) -> Self {
        self.input_examples
            .get_or_insert_with(Vec::new)
            .push(ToolInputExample::new(input));
        self
    }

    /// Set strict mode.
    pub fn with_strict(mut self, strict: bool) -> Self {
        self.strict = Some(strict);
        self
    }

    /// Set provider options.
    pub fn with_provider_options(mut self, options: ProviderOptions) -> Self {
        self.provider_options = Some(options);
        self
    }
}

#[cfg(test)]
#[path = "function_tool.test.rs"]
mod tests;
