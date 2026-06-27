//! vercel-ai-anthropic — Anthropic provider for Vercel AI SDK in Rust.
//!
//! This crate implements the Anthropic provider for the Vercel AI SDK v4 specification,
//! providing access to Claude models through a unified interface.
//!
//! # Quick Start
//!
//! ```ignore
//! use vercel_ai_anthropic::{anthropic, AnthropicProviderSettings, create_anthropic};
//!
//! // Default provider (uses ANTHROPIC_API_KEY env var)
//! let provider = anthropic();
//!
//! // Messages API
//! let model = provider.messages("claude-sonnet-4-5");
//! ```

// Foundation
pub mod anthropic_config;
pub mod anthropic_error;
pub mod anthropic_metadata;
pub mod anthropic_provider;

// Model implementations
pub mod messages;

// Provider tools
pub mod tool;

// Cache control and utilities
pub mod beta_capabilities;
pub mod beta_resolver;
pub mod cache_control;
pub mod cache_placement;
pub mod cache_policy;
pub mod forward_container_id;
pub mod provider_options;
pub mod sanitize_json_schema;

// Re-exports
pub use anthropic_config::AdapterAccountKind;
pub use anthropic_config::AnthropicConfig;
pub use anthropic_config::AnthropicModelCapabilities;
pub use anthropic_config::ProviderTopology;
pub use anthropic_provider::AnthropicProvider;
pub use anthropic_provider::AnthropicProviderSettings;
pub use anthropic_provider::anthropic;
pub use anthropic_provider::create_anthropic;
pub use beta_capabilities::CLAUDE_CODE_BASELINE;
pub use beta_capabilities::map_capability;
pub use beta_resolver::ResolvedBetas;
pub use beta_resolver::resolve as resolve_betas;
pub use beta_resolver::should_emit_context_management;
pub use cache_control::CacheControlValidator;
pub use cache_placement::attach_marker_at;
pub use cache_placement::build_cache_control_value;
pub use cache_placement::compute_marker_index_post_group;
pub use cache_policy::CachePolicy;
pub use forward_container_id::forward_anthropic_container_id_from_last_step;
pub use messages::AnthropicMessagesLanguageModel;
pub use provider_options::AnthropicProviderOptionsConfig;
pub use provider_options::ProviderOptionsError;
pub use provider_options::parse_provider_options;
