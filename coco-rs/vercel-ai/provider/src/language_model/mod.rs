//! Language model module.
//!
//! This module provides language model types organized by version.

pub mod v4;

// Re-export v4 types at this level for backward compatibility
pub use v4::FinishReason;
pub use v4::InputTokens;
pub use v4::LanguageModelV4;
pub use v4::LanguageModelV4CallOptions;
pub use v4::LanguageModelV4CallSettings;
pub use v4::LanguageModelV4GenerateResult;
pub use v4::LanguageModelV4Message;
pub use v4::LanguageModelV4Prompt;
pub use v4::LanguageModelV4ProviderTool;
pub use v4::LanguageModelV4Request;
pub use v4::LanguageModelV4Response;
pub use v4::LanguageModelV4StreamPart;
pub use v4::LanguageModelV4StreamResponse;
pub use v4::LanguageModelV4StreamResult;
pub use v4::LanguageModelV4Tool;
pub use v4::LanguageModelV4ToolChoice;
pub use v4::OutputTokens;
pub use v4::PromptBuilder;
pub use v4::ReasoningLevel;
pub use v4::ResponseFormat;
pub use v4::Source;
pub use v4::SourceType;
pub use v4::StreamError;
pub use v4::ToolApprovalRequest;
pub use v4::UnifiedFinishReason;
pub use v4::Usage;
pub use v4::WireTap;
pub use v4::WireTapHandle;
