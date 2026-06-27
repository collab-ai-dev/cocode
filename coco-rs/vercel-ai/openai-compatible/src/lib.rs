//! vercel-ai-openai-compatible — OpenAI-compatible provider for Vercel AI SDK in Rust.
//!
//! This crate implements a generic OpenAI-compatible provider for the Vercel AI SDK v4
//! specification, supporting any API that follows the OpenAI API protocol (xAI, Groq,
//! Together, Fireworks, DeepSeek, etc.).
//!
//! Unlike the `vercel-ai-openai` crate, this has:
//! - No OpenAI-specific features (capabilities detection, organization/project headers, Responses API)
//! - Additional extensibility: `MetadataExtractor` trait, `transform_request_body` hook, `query_params`
//! - Generic API key env var (not hardcoded to `OPENAI_API_KEY`)
//! - Reasoning support in responses (`reasoning_content` / `reasoning` fields)
//!
//! # Quick Start
//!
//! ```ignore
//! use vercel_ai_openai_compatible::{
//!     create_openai_compatible, OpenAICompatibleProviderSettings,
//! };
//!
//! let provider = create_openai_compatible(OpenAICompatibleProviderSettings {
//!     name: Some("xai".into()),
//!     base_url: Some("https://api.x.ai/v1".into()),
//!     api_key_env_var: Some("XAI_API_KEY".into()),
//!     api_key_description: Some("xAI".into()),
//!     ..Default::default()
//! });
//!
//! // Chat Completions API (default for language_model())
//! let chat = provider.chat("grok-2");
//!
//! // Embeddings
//! let embeddings = provider.embedding("text-embedding-3-small");
//!
//! // Images
//! let images = provider.image("dall-e-3");
//! ```

// Foundation
pub mod metadata_extractor;
pub mod openai_compatible_config;
pub mod openai_compatible_error;
pub mod openai_compatible_provider;
pub mod openai_compatible_provider_settings;
pub mod provider_options;
pub mod provider_options_key;
pub mod provider_profile;

// Model implementations
pub mod chat;
pub mod completion;
pub mod embedding;
pub mod image;

// Re-exports
pub use metadata_extractor::MetadataExtractor;
pub use metadata_extractor::StreamMetadataExtractor;
pub use openai_compatible_config::OpenAICompatibleConfig;
pub use openai_compatible_config::SupportedUrlsFn;
pub use openai_compatible_provider::OpenAICompatibleProvider;
pub use openai_compatible_provider::create_openai_compatible;
pub use openai_compatible_provider_settings::OpenAICompatibleProviderSettings;
pub use provider_options::OpenAICompatibleProviderOptionsConfig;
pub use provider_options::PromptTokensTotalSemantics;
pub use provider_options::parse_provider_options;
pub use provider_profile::OpenAICompatibleProviderProfile;

// Provider options key utilities
pub use provider_options_key::effective_provider_options_key;
pub use provider_options_key::get_effective_provider_options;
pub use provider_options_key::to_camel_case;
pub use provider_options_key::warn_if_deprecated_provider_options_key;

// Model type re-exports
pub use chat::OpenAICompatibleChatLanguageModel;
pub use completion::OpenAICompatibleCompletionLanguageModel;
pub use embedding::OpenAICompatibleEmbeddingModel;
pub use image::OpenAICompatibleImageModel;
