//! vercel-ai-groq — Groq provider for the Vercel AI SDK in Rust.
//!
//! A faithful port of `@ai-sdk/groq`, implemented against the coco-rs
//! `vercel-ai-provider` v4 traits and reusing `vercel-ai-provider-utils`
//! transport primitives.
//!
//! Groq is OpenAI-wire compatible but diverges from the generic
//! `openai-compatible` provider in ways this crate handles natively:
//! - reasoning surfaces through the `reasoning` field (both response + stream);
//! - streaming usage arrives under `x_groq.usage`, not the top-level `usage`;
//! - request options: `reasoning_format`, `reasoning_effort`, `service_tier`;
//! - the `groq.browser_search` provider tool (supported models only);
//! - a speech-to-text transcription model.
//!
//! # Quick Start
//!
//! ```ignore
//! use vercel_ai_groq::{create_groq, GroqProviderSettings};
//!
//! let provider = create_groq(GroqProviderSettings {
//!     api_key: Some("gsk_...".into()),
//!     ..Default::default()
//! });
//!
//! let chat = provider.chat("llama-3.3-70b-versatile");
//! let transcription = provider.transcription("whisper-large-v3");
//! ```

// Foundation
pub mod convert_groq_usage;
pub mod groq_browser_search_models;
pub mod groq_config;
pub mod groq_error;
pub mod groq_provider;
pub mod map_groq_finish_reason;

// Models & tools
pub mod chat;
pub mod tool;
pub mod transcription;

// Re-exports
pub use convert_groq_usage::GroqUsage;
pub use convert_groq_usage::convert_groq_usage;
pub use groq_browser_search_models::BROWSER_SEARCH_SUPPORTED_MODELS;
pub use groq_browser_search_models::is_browser_search_supported_model;
pub use groq_config::GroqConfig;
pub use groq_error::GroqErrorData;
pub use groq_error::GroqFailedResponseHandler;
pub use groq_provider::GroqProvider;
pub use groq_provider::GroqProviderSettings;
pub use groq_provider::create_groq;
pub use map_groq_finish_reason::map_groq_finish_reason;

pub use chat::GroqChatLanguageModel;
pub use chat::GroqChatProviderOptions;
pub use tool::BROWSER_SEARCH_TOOL_ID;
pub use tool::browser_search;
pub use transcription::GroqTranscriptionModel;
pub use transcription::GroqTranscriptionProviderOptions;
