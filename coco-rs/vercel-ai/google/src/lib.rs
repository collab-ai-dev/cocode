//! Google Generative AI provider for Vercel AI SDK (Rust).
//!
//! This crate implements the Google Generative AI (Gemini) provider for the
//! Vercel AI SDK v4 specification. It supports language models, embedding models,
//! image generation, and video generation.
//!
//! # Quick Start
//!
//! ```ignore
//! use vercel_ai_google::{google, create_google_generative_ai, GoogleGenerativeAIProviderSettings};
//! use vercel_ai_provider::ProviderV4;
//!
//! // Default provider (uses GOOGLE_GENERATIVE_AI_API_KEY env var)
//! let provider = google();
//! let model = provider.language_model("gemini-2.0-flash").unwrap();
//!
//! // Custom provider
//! let provider = create_google_generative_ai(GoogleGenerativeAIProviderSettings {
//!     api_key: Some("your-key".to_string()),
//!     ..Default::default()
//! });
//! ```

// Internal modules
pub mod convert_google_generative_ai_usage;
pub mod convert_json_schema_to_openapi_schema;
pub mod convert_to_google_generative_ai_messages;
pub mod get_model_path;
pub mod google_error;
pub mod google_generative_ai_embedding_model;
pub mod google_generative_ai_embedding_options;
pub mod google_generative_ai_image_model;
pub mod google_generative_ai_image_options;
pub mod google_generative_ai_image_settings;
pub mod google_generative_ai_language_model;
pub mod google_generative_ai_options;
pub mod google_generative_ai_prompt;
pub mod google_generative_ai_video_model;
pub mod google_generative_ai_video_settings;
pub mod google_json_accumulator;
pub mod google_prepare_tools;
pub mod google_provider;
pub mod google_supported_file_url;
pub mod google_tools;
pub mod map_google_generative_ai_finish_reason;
pub mod tool;

// Re-export key types

// Provider factory (primary API)
pub use google_provider::GoogleGenerativeAIProvider;
pub use google_provider::GoogleGenerativeAIProviderSettings;
pub use google_provider::create_google_generative_ai;
pub use google_provider::google;

// Models
pub use google_generative_ai_embedding_model::GoogleGenerativeAIEmbeddingModel;
pub use google_generative_ai_embedding_model::GoogleGenerativeAIEmbeddingModelConfig;
pub use google_generative_ai_image_model::GoogleGenerativeAIImageModel;
pub use google_generative_ai_image_model::GoogleGenerativeAIImageModelConfig;
pub use google_generative_ai_language_model::GoogleGenerativeAILanguageModel;
pub use google_generative_ai_language_model::GoogleGenerativeAILanguageModelConfig;
// Reusable Gemini-wire building blocks for sibling transports
// (`vercel-ai-google-codeassist`): the parsed response type, the SSE stream
// factory, and its envelope mode.
pub use google_generative_ai_language_model::ChunkEnvelope;
pub use google_generative_ai_language_model::GoogleGenerateContentResponse;
pub use google_generative_ai_language_model::create_google_stream;
pub use google_generative_ai_video_model::GoogleGenerativeAIVideoModel;
pub use google_generative_ai_video_model::GoogleGenerativeAIVideoModelConfig;

// Options and settings
pub use google_generative_ai_embedding_options::*;
pub use google_generative_ai_image_settings::GoogleGenerativeAIImageSettings;
pub use google_generative_ai_options::*;
pub use google_generative_ai_video_settings::GoogleGenerativeAIVideoSettings;

// Prompt types
pub use google_generative_ai_prompt::*;

// Utilities
pub use convert_google_generative_ai_usage::GoogleTokenDetail;
pub use convert_google_generative_ai_usage::GoogleUsageMetadata;
pub use convert_google_generative_ai_usage::convert_usage;
pub use convert_json_schema_to_openapi_schema::convert_json_schema_to_openapi_schema;
pub use convert_to_google_generative_ai_messages::ConvertOptions;
pub use convert_to_google_generative_ai_messages::convert_to_google_generative_ai_messages;
pub use get_model_path::get_model_path;
pub use google_error::GoogleErrorData;
pub use google_error::GoogleFailedResponseHandler;
pub use google_prepare_tools::PreparedTools;
pub use google_prepare_tools::prepare_tools;
pub use google_supported_file_url::is_supported_file_url;
pub use map_google_generative_ai_finish_reason::map_finish_reason;

// Tool constructors
pub use google_tools::*;
