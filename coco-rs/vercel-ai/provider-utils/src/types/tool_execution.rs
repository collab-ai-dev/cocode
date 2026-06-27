//! Tool execution types.
//!
//! Types for defining and managing executable tools.

use std::any::Any;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value as JSONValue;
use tokio_util::sync::CancellationToken;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::JSONSchema;
use vercel_ai_provider::LanguageModelV4Message as ModelMessage;
use vercel_ai_provider::language_model::v4::function_tool::LanguageModelV4FunctionTool;

/// Additional options that are sent into each tool call.
#[derive(Debug, Clone)]
pub struct ToolExecutionOptions {
    /// The ID of the tool call.
    /// You can use it e.g. when sending tool-call related information with stream data.
    pub tool_call_id: String,
    /// Messages that were sent to the language model to initiate the response
    /// that contained the tool call. The messages **do not** include the system
    /// prompt nor the assistant response that contained the tool call.
    pub messages: Vec<ModelMessage>,
    /// An optional abort signal that indicates that the overall operation should be aborted.
    pub abort_signal: Option<CancellationToken>,
    /// User-defined context.
    ///
    /// Treat the context object as immutable inside tools.
    /// Mutating the context object can lead to race conditions and unexpected results
    /// when tools are called in parallel.
    ///
    /// Experimental (can break in patch releases).
    pub experimental_context: Option<Arc<dyn Any + Send + Sync>>,
}

impl ToolExecutionOptions {
    /// Create new tool execution options with just a tool call ID.
    pub fn new(tool_call_id: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            messages: Vec::new(),
            abort_signal: None,
            experimental_context: None,
        }
    }

    /// Set the messages.
    pub fn with_messages(mut self, messages: Vec<ModelMessage>) -> Self {
        self.messages = messages;
        self
    }

    /// Set the abort signal.
    pub fn with_abort_signal(mut self, signal: CancellationToken) -> Self {
        self.abort_signal = Some(signal);
        self
    }

    /// Set the experimental context.
    pub fn with_context<T: Any + Send + Sync + 'static>(mut self, context: T) -> Self {
        self.experimental_context = Some(Arc::new(context));
        self
    }

    /// Set the experimental context from a shared object.
    pub fn with_context_arc(mut self, context: Arc<dyn Any + Send + Sync>) -> Self {
        self.experimental_context = Some(context);
        self
    }

    /// Get the context as a specific type.
    pub fn get_context<T: Clone + 'static>(&self) -> Option<T> {
        self.experimental_context
            .as_ref()
            .and_then(|c| c.downcast_ref::<T>())
            .cloned()
    }
}

impl Default for ToolExecutionOptions {
    fn default() -> Self {
        Self::new("")
    }
}

/// Type alias for tool handler function.
pub type ToolHandler = Arc<
    dyn Fn(
            JSONValue,
            ToolExecutionOptions,
        ) -> Pin<Box<dyn Future<Output = Result<JSONValue, AISdkError>> + Send>>
        + Send
        + Sync,
>;

/// A tool that can be executed.
#[async_trait]
pub trait ExecutableTool: Send + Sync {
    /// Get the tool definition.
    fn definition(&self) -> &LanguageModelV4FunctionTool;

    /// Execute the tool with the given input and options.
    async fn execute(
        &self,
        input: JSONValue,
        options: ToolExecutionOptions,
    ) -> Result<JSONValue, AISdkError>;
}

/// A simple executable tool with a function pointer.
pub struct SimpleTool {
    /// The tool definition.
    pub definition: LanguageModelV4FunctionTool,
    /// The handler function.
    pub handler: ToolHandler,
}

impl SimpleTool {
    /// Create a new simple tool.
    pub fn new<F, Fut>(definition: LanguageModelV4FunctionTool, handler: F) -> Self
    where
        F: Fn(JSONValue, ToolExecutionOptions) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<JSONValue, AISdkError>> + Send + 'static,
    {
        Self {
            definition,
            handler: Arc::new(move |input, options| Box::pin(handler(input, options))),
        }
    }

    /// Create a tool with the given name.
    pub fn with_name(name: impl Into<String>) -> ToolBuilder {
        ToolBuilder::new(name)
    }
}

#[async_trait]
impl ExecutableTool for SimpleTool {
    fn definition(&self) -> &LanguageModelV4FunctionTool {
        &self.definition
    }

    async fn execute(
        &self,
        input: JSONValue,
        options: ToolExecutionOptions,
    ) -> Result<JSONValue, AISdkError> {
        (self.handler)(input, options).await
    }
}

/// Builder for SimpleTool.
pub struct ToolBuilder {
    name: String,
    description: Option<String>,
    parameters: Option<JSONSchema>,
}

impl ToolBuilder {
    /// Create a new builder.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            parameters: None,
        }
    }

    /// Set the description.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the parameters schema.
    pub fn parameters(mut self, parameters: JSONSchema) -> Self {
        self.parameters = Some(parameters);
        self
    }

    /// Build the tool with a handler.
    pub fn handler<F, Fut>(self, handler: F) -> SimpleTool
    where
        F: Fn(JSONValue, ToolExecutionOptions) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<JSONValue, AISdkError>> + Send + 'static,
    {
        let input_schema = self
            .parameters
            .unwrap_or_else(|| serde_json::json!({ "type": "object" }));
        let mut tool = LanguageModelV4FunctionTool::new(self.name, input_schema);
        if let Some(desc) = self.description {
            tool.description = Some(desc);
        }
        SimpleTool::new(tool, handler)
    }
}

/// Type alias for inferring tool input type.
///
/// In Rust, types are inferred at compile time, so this is primarily for documentation.
pub type InferToolInput<T> = T;

/// Type alias for inferring tool output type.
///
/// In Rust, types are inferred at compile time, so this is primarily for documentation.
pub type InferToolOutput<T> = T;

/// A registry of executable tools.
#[derive(Default)]
pub struct ToolRegistry {
    /// Tools by name.
    pub tools: HashMap<String, Arc<dyn ExecutableTool>>,
}

impl ToolRegistry {
    /// Create a new tool registry.
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool.
    pub fn register(&mut self, tool: Arc<dyn ExecutableTool>) {
        let name = tool.definition().name.clone();
        self.tools.insert(name, tool);
    }

    /// Get a tool by name.
    pub fn get(&self, name: &str) -> Option<&Arc<dyn ExecutableTool>> {
        self.tools.get(name)
    }

    /// Get all tool definitions.
    pub fn definitions(&self) -> Vec<&LanguageModelV4FunctionTool> {
        self.tools.values().map(|t| t.definition()).collect()
    }

    /// Execute a tool by name.
    pub async fn execute(
        &self,
        name: &str,
        input: JSONValue,
        options: ToolExecutionOptions,
    ) -> Result<JSONValue, AISdkError> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| AISdkError::new(format!("Tool '{name}' not found")))?;
        tool.execute(input, options).await
    }
}

#[cfg(test)]
#[path = "tool_execution.test.rs"]
mod tests;
