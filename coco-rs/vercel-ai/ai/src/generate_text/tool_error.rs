//! Tool error types for generate_text.
//!
//! This module provides error types for tool execution failures,
//! covering both static and dynamic tool error variants.

use std::fmt;

use vercel_ai_provider::ProviderMetadata;

/// Error that occurred during tool execution.
#[derive(Debug, Clone)]
pub struct ToolError {
    /// The tool call ID that caused the error.
    pub tool_call_id: String,
    /// The tool name that caused the error.
    pub tool_name: String,
    /// The error value (opaque JSON).
    pub error: serde_json::Value,
    /// The tool input that was provided.
    pub input: serde_json::Value,
    /// Whether the tool was provider-executed.
    pub provider_executed: Option<bool>,
    /// Provider-specific metadata.
    pub provider_metadata: Option<ProviderMetadata>,
    /// Whether this is a dynamic tool error (true) or static (false).
    pub dynamic: bool,
    /// Title for the tool error.
    pub title: Option<String>,
}

impl ToolError {
    /// Create a new tool error.
    pub fn new(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        error: serde_json::Value,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            error,
            input: serde_json::Value::Null,
            provider_executed: None,
            provider_metadata: None,
            dynamic: false,
            title: None,
        }
    }

    /// Create a tool error from a message string.
    pub fn from_message(message: impl Into<String>) -> Self {
        Self {
            tool_call_id: String::new(),
            tool_name: String::new(),
            error: serde_json::Value::String(message.into()),
            input: serde_json::Value::Null,
            provider_executed: None,
            provider_metadata: None,
            dynamic: false,
            title: None,
        }
    }

    /// Set the tool call ID.
    pub fn with_tool_call_id(mut self, id: impl Into<String>) -> Self {
        self.tool_call_id = id.into();
        self
    }

    /// Set the tool name.
    pub fn with_tool_name(mut self, name: impl Into<String>) -> Self {
        self.tool_name = name.into();
        self
    }

    /// Set the input.
    pub fn with_input(mut self, input: serde_json::Value) -> Self {
        self.input = input;
        self
    }

    /// Set whether the tool was provider-executed.
    pub fn with_provider_executed(mut self, provider_executed: bool) -> Self {
        self.provider_executed = Some(provider_executed);
        self
    }

    /// Set provider metadata.
    pub fn with_provider_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.provider_metadata = Some(metadata);
        self
    }

    /// Mark as dynamic tool error.
    pub fn as_dynamic(mut self) -> Self {
        self.dynamic = true;
        self
    }

    /// Set the title.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Get the error as a string message (convenience accessor).
    pub fn message(&self) -> String {
        match &self.error {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = self.message();
        if self.tool_name.is_empty() {
            write!(f, "Tool error: {msg}")
        } else if self.tool_call_id.is_empty() {
            write!(f, "Tool '{}' error: {msg}", self.tool_name)
        } else {
            write!(
                f,
                "Tool '{}' ({}) error: {msg}",
                self.tool_name, self.tool_call_id
            )
        }
    }
}

impl std::error::Error for ToolError {}

/// Result of a tool execution that may fail.
pub type ToolResult<T> = Result<T, ToolError>;

/// Create a tool error from a message.
pub fn tool_error(message: impl Into<String>) -> ToolError {
    ToolError::from_message(message)
}

/// Create a tool error with tool context.
pub fn tool_error_with_context(
    tool_call_id: impl Into<String>,
    tool_name: impl Into<String>,
    message: impl Into<String>,
) -> ToolError {
    ToolError::new(
        tool_call_id,
        tool_name,
        serde_json::Value::String(message.into()),
    )
}

#[cfg(test)]
#[path = "tool_error.test.rs"]
mod tests;
