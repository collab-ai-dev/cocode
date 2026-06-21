//! vercel-ai-openai — OpenAI provider for Vercel AI SDK in Rust.
//!
//! This crate implements the OpenAI provider for the Vercel AI SDK v4 specification,
//! providing access to OpenAI models through a unified interface.
//!
//! # Quick Start
//!
//! ```ignore
//! use vercel_ai_openai::{openai, OpenAIProviderSettings, create_openai};
//!
//! // Default provider (uses OPENAI_API_KEY env var)
//! let provider = openai();
//!
//! // Chat Completions API
//! let chat = provider.chat("gpt-4o");
//!
//! // Responses API (default for language_model())
//! let responses = provider.responses("gpt-4o");
//!
//! // Embeddings
//! let embeddings = provider.embedding("text-embedding-3-small");
//!
//! // Images
//! let images = provider.image("dall-e-3");
//! ```

// Foundation
pub mod openai_auth;
pub mod openai_capabilities;
pub mod openai_config;
pub mod openai_error;
pub mod openai_provider;
pub mod provider_options;

// Model implementations
pub mod chat;
pub mod completion;
pub mod embedding;
pub mod image;
pub mod responses;
pub mod speech;
pub mod transcription;

// Provider tools
pub mod tool;

// Re-exports
pub use openai_auth::CHATGPT_CODEX_BASE_URL;
pub use openai_auth::ChatGptCreds;
pub use openai_auth::ChatGptCredsSupplier;
pub use openai_auth::DEFAULT_ORIGINATOR;
pub use openai_auth::OpenAIAuth;
pub use openai_capabilities::OpenAIModelCapabilities;
pub use openai_capabilities::SystemMessageMode;
pub use openai_capabilities::get_capabilities;
pub use openai_config::OpenAIConfig;
pub use openai_config::ResponsesStorePolicy;
pub use openai_provider::OpenAIProvider;
pub use openai_provider::OpenAIProviderSettings;
pub use openai_provider::create_openai;
pub use openai_provider::openai;
pub use provider_options::OpenAIProviderOptionsConfig;
pub use provider_options::ProviderOptionsError;
pub use provider_options::parse_provider_options;

// Model type re-exports
pub use chat::OpenAIChatLanguageModel;
pub use completion::OpenAICompletionLanguageModel;
pub use embedding::OpenAIEmbeddingModel;
pub use image::OpenAIImageModel;
pub use responses::OpenAIResponsesLanguageModel;
pub use speech::OpenAISpeechModel;
pub use transcription::OpenAITranscriptionModel;
