//! Language model V4 module.
//!
//! This module contains all types related to the V4 language model specification.

// Core modules
mod call_options;
mod call_settings;
pub mod generate_result;
mod language_model_v4;
mod provider_tool;
mod stream_result;
pub mod tool;

// New type modules
pub mod content;
pub mod data_content;
pub mod file;
pub mod finish_reason;
pub mod function_tool;
pub mod prompt;
pub mod reasoning;
pub mod response_metadata;
pub mod source;
pub mod stream;
pub mod text;
pub mod tool_approval_request;
pub mod tool_call;
pub mod tool_choice;
pub mod tool_result;
pub mod usage;
pub mod wire_tap;

// Re-export main types
pub use call_options::LanguageModelV4CallOptions;
pub use call_options::ReasoningLevel;
pub use call_options::ResponseFormat;
pub use call_settings::LanguageModelV4CallSettings;
pub use generate_result::LanguageModelV4GenerateResult;
pub use generate_result::LanguageModelV4Request;
pub use generate_result::LanguageModelV4Response;
pub use language_model_v4::LanguageModelV4;
pub use provider_tool::LanguageModelV4ProviderTool;
pub use stream_result::LanguageModelV4StreamResponse;
pub use stream_result::LanguageModelV4StreamResult;

// Re-export from usage module
pub use usage::InputTokens;
pub use usage::OutputTokens;
pub use usage::Usage;

// Re-export from finish_reason module
pub use finish_reason::FinishReason;
pub use finish_reason::UnifiedFinishReason;

// Re-export from prompt module
pub use prompt::LanguageModelV4Message;
pub use prompt::LanguageModelV4Prompt;
pub use prompt::PromptBuilder;

// Re-export from stream module
pub use stream::LanguageModelV4StreamPart;
pub use stream::StreamError;

// Re-export from wire_tap module
pub use wire_tap::WireTap;
pub use wire_tap::WireTapHandle;

// Re-export new content types
pub use content::LanguageModelV4Content;
pub use data_content::LanguageModelV4DataContent;
pub use file::LanguageModelV4File;
pub use file::LanguageModelV4FileData;
pub use function_tool::LanguageModelV4FunctionTool;
pub use function_tool::ToolInputExample;
pub use reasoning::LanguageModelV4Reasoning;
pub use response_metadata::LanguageModelV4ResponseMetadata;
pub use source::LanguageModelV4Source;
pub use source::SourceType;
pub use text::LanguageModelV4Text;
pub use tool::LanguageModelV4Tool;
pub use tool_approval_request::LanguageModelV4ToolApprovalRequest;
pub use tool_call::LanguageModelV4ToolCall;
pub use tool_choice::LanguageModelV4ToolChoice;
pub use tool_result::LanguageModelV4ToolResult;

// Legacy re-exports for backward compatibility (from stream.rs)
pub use stream::File;
pub use stream::Source;
pub use stream::ToolApprovalRequest;
