//! vercel-ai-provider - Vercel AI SDK provider types for Rust
//!
//! This crate provides type definitions matching `@ai-sdk/provider` v4 specification.
//! It is a standalone types crate with no dependencies on other coco crates.
//!
//! # Key Types
//!
//! - [`LanguageModelV4`] - Trait for language model implementations
//! - [`EmbeddingModelV4`] - Trait for embedding model implementations
//! - [`ImageModelV4`] - Trait for image model implementations
//! - [`ProviderV4`] - Trait for provider implementations
//! - [`LanguageModelV4Prompt`] - Prompt type (vector of messages)
//!
//! # Content Types
//!
//! Content is structured as typed parts:
//! - [`UserContentPart`] - User message content (Text, File)
//! - [`AssistantContentPart`] - Assistant content (Text, File, Reasoning, ToolCall, ToolResult)
//! - [`ToolContentPart`] - Tool message content (ToolResult, ToolApprovalResponse)
//!
//! # Stream Events
//!
//! Streaming uses granular events with IDs:
//! - `TextStart`, `TextDelta`, `TextEnd`
//! - `ReasoningStart`, `ReasoningDelta`, `ReasoningEnd`
//! - `ToolInputStart`, `ToolInputDelta`, `ToolInputEnd`
//! - `ToolCall`, `ToolResult`
//!
//! # Example
//!
//! ```ignore
//! use vercel_ai_provider::{LanguageModelV4, LanguageModelV4CallOptions};
//!
//! async fn generate(model: &dyn LanguageModelV4, prompt: LanguageModelV4Prompt) {
//!     let result = model.do_generate(LanguageModelV4CallOptions {
//!         prompt,
//!         ..Default::default()
//!     }).await.unwrap();
//!     println!("Response: {:?}", result);
//! }
//! ```

// Core types kept at root (shared across model types)
pub mod content;
pub mod data_content;
pub mod files;
pub mod json_schema;
pub mod response_metadata;
pub mod tool;

// Versioned modules
pub mod embedding_model;
pub mod errors;
pub mod image_model;
pub mod json_value;
pub mod language_model;
pub mod provider;
pub mod shared;

// New model types
pub mod reranking_model;
pub mod speech_model;
pub mod transcription_model;
pub mod video_model;

// Middleware patterns
pub mod embedding_model_middleware;
pub mod image_model_middleware;
pub mod language_model_middleware;

// Re-export main types at crate root

// Error types
pub use errors::AISdkError;
pub use errors::APICallError;
pub use errors::EmptyResponseBodyError;
pub use errors::InvalidArgumentError;
pub use errors::InvalidPromptError;
pub use errors::InvalidResponseDataError;
pub use errors::JSONParseError;
pub use errors::LoadAPIKeyError;
pub use errors::LoadSettingError;
pub use errors::NoContentGeneratedError;
pub use errors::NoSuchModelError;
pub use errors::NoSuchProviderReferenceError;
pub use errors::ProviderError;
pub use errors::TooManyEmbeddingValuesForCallError;
pub use errors::TypeValidationContext;
pub use errors::TypeValidationError;
pub use errors::UnsupportedFunctionalityError;

// Shared types
pub use shared::FileRawData;
pub use shared::ProviderMetadata;
pub use shared::ProviderOptions;
pub use shared::SharedV4FileData;
pub use shared::Warning;

// Files interface
pub use files::FilesV4;
pub use files::FilesV4UploadData;
pub use files::FilesV4UploadFileCallOptions;
pub use files::FilesV4UploadFileResult;

// Content types
pub use content::AssistantContentPart;
pub use content::CustomPart;
pub use content::FilePart;
pub use content::ReasoningFilePart;
pub use content::ReasoningPart;
pub use content::SharedV4ProviderReference;
pub use content::TextPart;
pub use content::ToolCallPart;
pub use content::ToolContentPart;
pub use content::ToolInputInvalidReason;
pub use content::ToolResultContent;
pub use content::ToolResultContentPart;
pub use content::ToolResultPart;
pub use content::UserContentPart;

// Data content (legacy — prefer SharedV4FileData for new code)
pub use data_content::DataContent;

// Prompt types
pub use language_model::LanguageModelV4Message;
pub use language_model::LanguageModelV4Prompt;

// Tool types (legacy aliases pointing to v4 types)
pub type ToolDefinitionV4 = language_model::v4::function_tool::LanguageModelV4FunctionTool;
pub type ToolChoice = language_model::v4::tool_choice::LanguageModelV4ToolChoice;
pub use language_model::v4::function_tool::ToolInputExample;
pub use tool::ToolInvocation;

// Usage types
pub use language_model::InputTokens;
pub use language_model::OutputTokens;
pub use language_model::Usage;

// Finish reason
pub use language_model::FinishReason;
pub use language_model::UnifiedFinishReason;

// JSON Schema
pub use json_schema::JSONSchema;

// JSON Value
pub use json_value::JSONArray;
pub use json_value::JSONObject;
pub use json_value::JSONValue;

// Language model file data
pub use language_model::v4::file::LanguageModelV4FileData;
pub use language_model::v4::tool_call::LanguageModelV4ToolCall;
pub use language_model::v4::tool_result::LanguageModelV4ToolResult;

// Model traits
pub use embedding_model::EmbeddingModelV4;
pub use embedding_model::EmbeddingModelV4CallOptions;
pub use embedding_model::EmbeddingModelV4EmbedResult;
pub use embedding_model::EmbeddingType;
pub use embedding_model::EmbeddingUsage;
pub use embedding_model::EmbeddingValue;
pub use image_model::ImageModelV4;
pub use language_model::LanguageModelV4;
pub use language_model::LanguageModelV4CallOptions;
pub use language_model::LanguageModelV4GenerateResult;
pub use language_model::LanguageModelV4ProviderTool;
pub use language_model::LanguageModelV4Request;
pub use language_model::LanguageModelV4Response;
pub use language_model::LanguageModelV4StreamResponse;
pub use language_model::LanguageModelV4StreamResult;
pub use language_model::LanguageModelV4Tool;
pub use language_model::LanguageModelV4ToolChoice;
pub use language_model::ReasoningLevel;
pub use language_model::ResponseFormat;
pub use provider::ProviderV4;
pub use provider::SimpleProvider;

// Stream
pub use content::SourceType;
pub use language_model::LanguageModelV4StreamPart;
pub use language_model::Source;
pub use language_model::StreamError;
pub use language_model::ToolApprovalRequest;
pub use language_model::WireTap;
pub use language_model::WireTapHandle;

// Response metadata
pub use response_metadata::ResponseMetadata;

// New model types - Speech
pub use speech_model::SpeechModelV4;
pub use speech_model::SpeechModelV4CallOptions;
pub use speech_model::SpeechModelV4Request;
pub use speech_model::SpeechModelV4Response;
pub use speech_model::SpeechModelV4Result;

// New model types - Transcription
pub use transcription_model::TranscriptionModelV4;
pub use transcription_model::TranscriptionModelV4CallOptions;
pub use transcription_model::TranscriptionModelV4Request;
pub use transcription_model::TranscriptionModelV4Response;
pub use transcription_model::TranscriptionModelV4Result;
pub use transcription_model::TranscriptionSegmentV4;

// New model types - Reranking
pub use reranking_model::RankedItem;
pub use reranking_model::RerankDocuments;
pub use reranking_model::RerankingModelV4;
pub use reranking_model::RerankingModelV4CallOptions;
pub use reranking_model::RerankingModelV4Response;
pub use reranking_model::RerankingModelV4Result;
pub use video_model::VideoDuration;
pub use video_model::VideoModelV4;
pub use video_model::VideoModelV4CallOptions;
pub use video_model::VideoModelV4Result;
pub use video_model::VideoSize;
pub use video_model::v4::VideoData;

// Image model types
pub use image_model::GeneratedImage;
pub use image_model::ImageData;
pub use image_model::ImageFileData;
pub use image_model::ImageModelV4CallOptions;
pub use image_model::ImageModelV4File;
pub use image_model::ImageModelV4GenerateResult;
pub use image_model::ImageQuality;
pub use image_model::ImageResponseFormat;
pub use image_model::ImageSize;
pub use image_model::ImageStyle;
pub use image_model::v4::ImageModelV4Response;
pub use image_model::v4::ImageModelV4Usage;

// Middleware types
pub use embedding_model_middleware::EmbeddingModelV4Middleware;
pub use image_model_middleware::ImageModelV4Middleware;
pub use language_model_middleware::LanguageModelV4Middleware;
