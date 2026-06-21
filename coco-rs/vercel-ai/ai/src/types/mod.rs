//! Types module - re-exports from provider crate.
//!
//! This module re-exports commonly used types from the provider crate
//! for convenience.

mod response_metadata;
mod usage;

// Re-export content types
pub use vercel_ai_provider::AssistantContentPart;
pub use vercel_ai_provider::DataContent;
pub use vercel_ai_provider::FilePart;
pub use vercel_ai_provider::ReasoningPart;
pub use vercel_ai_provider::TextPart;
pub use vercel_ai_provider::ToolCallPart;
pub use vercel_ai_provider::ToolContentPart;
pub use vercel_ai_provider::ToolResultContent;
pub use vercel_ai_provider::ToolResultContentPart;
pub use vercel_ai_provider::ToolResultPart;
pub use vercel_ai_provider::UserContentPart;

// Re-export message types
pub use vercel_ai_provider::LanguageModelV4Message as ModelMessage;
pub use vercel_ai_provider::LanguageModelV4Prompt as ModelPrompt;

// Re-export tool types
pub use vercel_ai_provider::LanguageModelV4Tool;
pub use vercel_ai_provider::LanguageModelV4ToolChoice;
pub use vercel_ai_provider::ToolInvocation;
pub use vercel_ai_provider::language_model::v4::function_tool::LanguageModelV4FunctionTool;

pub type ToolChoice = LanguageModelV4ToolChoice;

// Re-export usage types
pub use vercel_ai_provider::InputTokens;
pub use vercel_ai_provider::OutputTokens;
pub use vercel_ai_provider::Usage;

// Re-export finish reason
pub use vercel_ai_provider::FinishReason;

// Re-export JSON types
pub use vercel_ai_provider::JSONSchema;
pub use vercel_ai_provider::JSONValue;

// Re-export shared types
pub use vercel_ai_provider::ProviderMetadata;
pub use vercel_ai_provider::ProviderOptions;
pub use vercel_ai_provider::Warning;

// Re-export error types
pub use vercel_ai_provider::AISdkError;

// Re-export model traits
pub use vercel_ai_provider::EmbeddingModelV4;
pub use vercel_ai_provider::LanguageModelV4;
pub use vercel_ai_provider::ProviderV4;
pub use vercel_ai_provider::VideoModelV4;

// Re-export response types
pub use vercel_ai_provider::LanguageModelV4CallOptions;
pub use vercel_ai_provider::LanguageModelV4GenerateResult;
pub use vercel_ai_provider::LanguageModelV4StreamPart;
pub use vercel_ai_provider::LanguageModelV4StreamResult;
pub use vercel_ai_provider::ResponseFormat;

// Re-export embedding types
pub use vercel_ai_provider::EmbeddingModelV4CallOptions;
pub use vercel_ai_provider::EmbeddingModelV4EmbedResult;
pub use vercel_ai_provider::EmbeddingType;
pub use vercel_ai_provider::EmbeddingUsage;
pub use vercel_ai_provider::EmbeddingValue;

// Re-export stream types
pub use vercel_ai_provider::Source;
pub use vercel_ai_provider::SourceType;
pub use vercel_ai_provider::StreamError;

// Re-export from provider-utils
pub use vercel_ai_provider_utils::ExecutableTool;
pub use vercel_ai_provider_utils::SimpleTool;
pub use vercel_ai_provider_utils::ToolExecutionOptions;
pub use vercel_ai_provider_utils::ToolRegistry;

// Re-export response metadata
pub use response_metadata::ImageModelResponseMetadata;
pub use response_metadata::LanguageModelRequestMetadata;
pub use response_metadata::LanguageModelResponseMetadata;
pub use response_metadata::SpeechModelResponseMetadata;
pub use response_metadata::TranscriptionModelResponseMetadata;
pub use response_metadata::VideoModelResponseMetadata;

/// Type alias for call warnings.
pub type CallWarning = Warning;

// Re-export usage types
pub use usage::InputTokenDetails;
pub use usage::LanguageModelUsage;
pub use usage::OutputTokenDetails;
pub use usage::add_image_model_usage;
pub use usage::add_language_model_usage;
pub use usage::as_language_model_usage;
pub use usage::create_null_language_model_usage;

/// Type alias for image model usage (re-export from provider).
pub type ImageModelUsage = vercel_ai_provider::ImageModelV4Usage;
/// Type alias for embedding model usage (re-export from provider).
pub type EmbeddingModelUsage = vercel_ai_provider::EmbeddingUsage;
