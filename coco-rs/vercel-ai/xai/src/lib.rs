//! vercel-ai-xai — xAI (Grok) provider for the Vercel AI SDK in Rust.
//!
//! A faithful port of `@ai-sdk/xai`, scoped to the Chat Completions surface,
//! implemented against the coco-rs `vercel-ai-provider` v4 traits and reusing
//! `vercel-ai-provider-utils` transport primitives (shared `reqwest::Client` +
//! typed `ResponseHandler` + the shared `SseDecoder`).
//!
//! xAI is OpenAI-wire compatible but diverges from the generic
//! `openai-compatible` provider in ways this crate handles natively:
//! - reasoning surfaces through the `reasoning_content` field (response + stream);
//! - `max_completion_tokens` instead of `max_tokens`;
//! - `topK` / `frequencyPenalty` / `presencePenalty` / `stopSequences` are
//!   unsupported and produce warnings;
//! - `reasoning_effort` (incl. the literal `none`), gated per-model by
//!   [`supports_reasoning_effort`];
//! - `logprobs` / `top_logprobs` and `parallel_function_calling` options;
//! - Live-Search `citations` surface as URL `source` parts;
//! - streaming tool calls arrive complete in a single delta (like Mistral).
//!
//! # Quick Start
//!
//! ```ignore
//! use vercel_ai_xai::{create_xai, XaiProviderSettings};
//!
//! let provider = create_xai(XaiProviderSettings {
//!     api_key: Some("xai-...".into()),
//!     ..Default::default()
//! });
//!
//! let chat = provider.chat("grok-4.5");
//! ```

// Foundation
pub mod convert_xai_chat_usage;
pub mod map_xai_finish_reason;
pub mod remove_additional_properties;
pub mod supports_reasoning_effort;
pub mod xai_config;
pub mod xai_error;
pub mod xai_provider;

// Models
pub mod chat;
pub mod image;
pub mod responses;
pub mod speech;
pub mod transcription;
pub mod video;

// Re-exports
pub use convert_xai_chat_usage::XaiChatUsage;
pub use convert_xai_chat_usage::convert_xai_chat_usage;
pub use map_xai_finish_reason::map_xai_finish_reason;
pub use remove_additional_properties::remove_additional_properties_false;
pub use supports_reasoning_effort::supports_reasoning_effort;
pub use xai_config::XaiConfig;
pub use xai_error::XaiErrorData;
pub use xai_error::XaiFailedResponseHandler;
pub use xai_provider::XaiProvider;
pub use xai_provider::XaiProviderSettings;
pub use xai_provider::create_xai;

pub use chat::XaiChatLanguageModel;
pub use chat::XaiChatProviderOptions;

pub use responses::XaiResponsesLanguageModel;
pub use responses::XaiResponsesProviderOptions;

pub use image::XaiImageModel;
pub use image::XaiImageProviderOptions;

pub use video::XaiVideoModel;
pub use video::XaiVideoProviderOptions;

pub use speech::XaiSpeechModel;
pub use speech::XaiSpeechProviderOptions;

pub use transcription::XaiTranscriptionModel;
pub use transcription::XaiTranscriptionProviderOptions;
