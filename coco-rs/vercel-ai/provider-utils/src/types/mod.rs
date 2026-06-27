//! Types module.
//!
//! This module contains shared type definitions used across the SDK.

mod content_part;
mod model_message;
mod tool;

// New types
mod assistant_model_message;
mod data_content;
mod provider_options;
mod system_model_message;
mod tool_approval_request;
mod tool_approval_response;
mod tool_model_message;
mod user_model_message;

// Tool execution types
mod tool_execution;

pub use content_part::*;
pub use model_message::*;
pub use tool::*;

// Export new types
pub use assistant_model_message::AssistantContent;
pub use assistant_model_message::AssistantModelMessage;
pub use data_content::DataContent;
pub use provider_options::ProviderOptions;
pub use system_model_message::SystemModelMessage;
pub use tool_approval_request::ToolApprovalRequest;
pub use tool_approval_response::ToolApprovalResponse;
pub use tool_model_message::ToolApprovalResponse as ToolMessageApprovalResponse;
pub use tool_model_message::ToolContent;
pub use tool_model_message::ToolContentPart;
pub use tool_model_message::ToolModelMessage;
pub use user_model_message::UserContent;
pub use user_model_message::UserModelMessage;

// Export tool execution types
pub use tool_execution::ExecutableTool;
pub use tool_execution::InferToolInput;
pub use tool_execution::InferToolOutput;
pub use tool_execution::SimpleTool;
pub use tool_execution::ToolBuilder;
pub use tool_execution::ToolExecutionOptions;
pub use tool_execution::ToolHandler;
pub use tool_execution::ToolRegistry;
