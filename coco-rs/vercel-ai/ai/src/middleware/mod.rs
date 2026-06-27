//! Middleware module for wrapping models with custom behavior.
//!
//! This module provides functions to wrap models with middleware that can
//! transform parameters, wrap generate/stream calls, and modify model behavior.

mod add_tool_input_examples_middleware;
mod default_embedding_settings_middleware;
mod default_settings_middleware;
mod extract_json_middleware;
mod extract_reasoning_middleware;
mod simulate_streaming_middleware;
mod wrap_embedding_model;
mod wrap_image_model;
mod wrap_language_model;
mod wrap_provider;

// Re-export middleware types from provider crate
pub use vercel_ai_provider::EmbeddingModelV4Middleware;
pub use vercel_ai_provider::ImageModelV4Middleware;
pub use vercel_ai_provider::LanguageModelV4Middleware;

// Re-export middleware functions
pub use add_tool_input_examples_middleware::add_tool_input_examples_middleware;
pub use default_embedding_settings_middleware::DefaultEmbeddingSettings;
pub use default_embedding_settings_middleware::default_embedding_settings_middleware;
pub use default_settings_middleware::DefaultSettings;
pub use default_settings_middleware::default_settings_middleware;
pub use extract_json_middleware::extract_json_middleware;
pub use extract_reasoning_middleware::extract_reasoning_middleware;
pub use simulate_streaming_middleware::simulate_streaming_middleware;
pub use wrap_embedding_model::EmbeddingMiddleware;
pub use wrap_embedding_model::wrap_embedding_model;
pub use wrap_image_model::ImageMiddleware;
pub use wrap_image_model::wrap_image_model;
pub use wrap_language_model::wrap_language_model;
pub use wrap_provider::wrap_provider;
