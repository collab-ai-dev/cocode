//! ByteDance Seedance video provider for Vercel AI SDK (Rust).
//!
//! This crate implements the ByteDance video provider for the Vercel AI SDK v4
//! specification. It supports video generation using the Seedance model family
//! via ByteDance's ModelArk API.
//!
//! # Quick Start
//!
//! ```ignore
//! use vercel_ai_bytedance::{bytedance, create_bytedance, ByteDanceProviderSettings};
//! use vercel_ai_provider::ProviderV4;
//!
//! // Default provider (uses ARK_API_KEY env var)
//! let provider = bytedance();
//! let model = provider.video_model("seedance-1-5-pro-251215").unwrap();
//!
//! // Custom provider
//! let provider = create_bytedance(ByteDanceProviderSettings {
//!     api_key: Some("your-key".to_string()),
//!     ..Default::default()
//! });
//! ```

// Internal modules
pub mod bytedance_config;
pub mod bytedance_error;
pub mod bytedance_provider;
pub mod bytedance_video_model;
pub mod bytedance_video_options;
pub mod bytedance_video_settings;

// Re-export key types

// Provider factory (primary API)
pub use bytedance_provider::ByteDanceProvider;
pub use bytedance_provider::ByteDanceProviderSettings;
pub use bytedance_provider::bytedance;
pub use bytedance_provider::create_bytedance;

// Models
pub use bytedance_video_model::ByteDanceVideoModel;
pub use bytedance_video_model::map_resolution;

// Config
pub use bytedance_config::ByteDanceVideoModelConfig;

// Options and settings
pub use bytedance_video_options::ByteDanceVideoProviderOptions;
pub use bytedance_video_settings::*;

// Error types
pub use bytedance_error::ByteDanceErrorData;
pub use bytedance_error::ByteDanceFailedResponseHandler;
