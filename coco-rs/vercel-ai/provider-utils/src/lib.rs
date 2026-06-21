//! vercel-ai-provider-utils - Vercel AI SDK provider utilities for Rust
//!
//! This crate provides utility functions for implementing AI providers
//! that follow the Vercel AI SDK v4 specification.
//!
//! # Key Utilities
//!
//! - [`load_api_key`] - API key loading from environment variables
//! - [`load_setting`] - Generic setting loading utilities
//! - [`post_json_to_api`] - POST JSON data to an API endpoint
//! - [`get_from_api`] - GET data from an API endpoint
//! - [`generate_id`] - Generate unique IDs
//! - [`combine_headers`] - Combine multiple header maps
//!
//! # Example
//!
//! ```ignore
//! use vercel_ai_provider_utils::{load_api_key, post_json_to_api, JsonResponseHandler};
//!
//! async fn call_api() -> Result<(), vercel_ai_provider::AISdkError> {
//!     let api_key = load_api_key(None, "MY_API_KEY", "My Provider")?;
//!
//!     let response = post_json_to_api(
//!         "https://api.example.com/v1/chat",
//!         Some({
//!             let mut headers = std::collections::HashMap::new();
//!             headers.insert("Authorization".to_string(), format!("Bearer {}", api_key));
//!             headers
//!         }),
//!         &serde_json::json!({ "prompt": "Hello" }),
//!         JsonResponseHandler::new(),
//!         DefaultErrorHandler,
//!         None,
//!     ).await?;
//!
//!     Ok(())
//! }
//! ```

// Module declarations
pub mod add_additional_properties_to_json_schema;
pub mod api;
pub mod data_uri;
pub mod delay;
pub mod download;
pub mod error_message;
pub mod extract_namespaced;
pub mod fetch;
pub mod form_data;
pub mod generate_id;
pub mod headers;
pub mod inject_json_instruction;
pub mod json;
pub mod json_repair;
pub mod json_schema_derive;
pub mod load_api_key;
pub mod load_setting;
pub mod map_reasoning_to_provider;
pub mod media_type;
pub mod openai_compatible_schema;
pub mod parse_provider_options;
pub mod resolve_full_media_type;
pub mod resolve_provider_reference;
pub mod response_handler;
pub mod schema;
pub mod streaming_tool_call_tracker;
pub mod strip_extension;
pub mod tool_execution;
pub mod tool_mapping;
pub mod types;
pub mod uint8_utils;
pub mod url;
pub mod user_agent;
pub mod validate_download_url;
pub mod validator;
pub mod version;
pub mod without_trailing_slash;

// Re-export main utilities
pub use api::ApiError;
pub use api::ApiResponse;
pub use api::ByteStream;
pub use api::DefaultErrorHandler;
pub use api::ErrorHandler;
pub use api::get_from_api;
pub use api::get_from_api_with_client;
pub use api::post_json_to_api;
pub use api::post_json_to_api_with_client;
pub use api::post_json_to_api_with_client_and_headers;
pub use api::post_json_to_api_with_client_and_headers_tapped;
pub use api::post_json_to_api_with_client_tapped;
pub use api::post_stream_to_api;
pub use api::post_stream_to_api_with_client;
pub use api::post_stream_to_api_with_client_and_headers;
pub use api::post_stream_to_api_with_client_and_headers_tapped;
pub use api::post_stream_to_api_with_client_tapped;
pub use api::tap_byte_stream;

pub use delay::delay;
pub use delay::parse_retry_after;

pub use error_message::get_error_message;

pub use extract_namespaced::ExtractExtras;
pub use extract_namespaced::extract_namespaced;

pub use fetch::Fetch;
pub use fetch::FetchOptions;

pub use generate_id::generate_id;

pub use headers::combine_headers;
pub use headers::extract_header;
pub use headers::normalize_headers;

pub use json::is_prototype_polluting_key;
pub use json::merge_json_value;
pub use json::parse_json;
pub use json::parse_json_event_stream;

pub use load_api_key::load_api_key;
pub use load_api_key::load_optional_api_key;

pub use load_setting::load_optional_setting;
pub use load_setting::load_setting;

// Re-export LoadAPIKeyError from provider crate for convenience
pub use vercel_ai_provider::LoadAPIKeyError;

pub use response_handler::JsonResponseHandler;
pub use response_handler::ResponseHandler;
pub use response_handler::StreamResponseHandler;
pub use response_handler::TextResponseHandler;

pub use schema::Schema;
pub use schema::ValidationError;
pub use schema::as_schema;
pub use schema::json_schema;

pub use url::is_url_supported;
pub use url::parse_data_url;

pub use tool_mapping::ToolMapping;
pub use tool_mapping::generate_tool_call_id;
pub use tool_mapping::parse_tool_call_id;

pub use data_uri::DataUri;
pub use data_uri::parse_data_uri;

pub use form_data::FormData;

pub use download::download_file;

pub use user_agent::build_user_agent;

pub use media_type::MediaType;
pub use media_type::media_type_from_extension;

pub use validator::validate_model_id;
pub use validator::validate_tool_name;

pub use strip_extension::strip_extension;
pub use strip_extension::strip_specific_extension;

pub use without_trailing_slash::normalize_url;
pub use without_trailing_slash::with_trailing_slash;
pub use without_trailing_slash::without_trailing_slash;

pub use map_reasoning_to_provider::is_custom_reasoning;
pub use map_reasoning_to_provider::map_reasoning_to_provider_budget;
pub use map_reasoning_to_provider::map_reasoning_to_provider_effort;

pub use validate_download_url::DownloadUrlError;
pub use validate_download_url::is_valid_download_url;
pub use validate_download_url::validate_download_url;

pub use inject_json_instruction::create_json_response_instruction;
pub use inject_json_instruction::inject_json_array_instruction;
pub use inject_json_instruction::inject_json_instruction;
pub use inject_json_instruction::inject_json_instruction_with_description;

pub use json_repair::RepairOutcome;
pub use json_repair::parse_tool_arguments_or_empty;
pub use json_repair::parse_with_repair;
pub use json_schema_derive::GeneratedSchema;
pub use json_schema_derive::add_required_fields;
pub use json_schema_derive::json_schema_from_type;
pub use json_schema_derive::merge_into_schema;
pub use json_schema_derive::schema_from_type;

pub use openai_compatible_schema::to_openai_compatible_schema;

// Tool execution utilities
pub use tool_execution::dynamic_tool;
pub use tool_execution::execute_tool;
pub use types::ExecutableTool;
pub use types::SimpleTool;
pub use types::ToolExecutionOptions;
pub use types::ToolRegistry;

// Streaming tool call tracking
pub use parse_provider_options::parse_provider_options;
pub use parse_provider_options::parse_provider_options_with_fallback;
pub use resolve_provider_reference::resolve_provider_reference;
pub use streaming_tool_call_tracker::StreamingToolCallDelta;
pub use streaming_tool_call_tracker::StreamingToolCallTracker;
pub use streaming_tool_call_tracker::StreamingToolCallTrackerOptions;
pub use streaming_tool_call_tracker::ToolCallDeltaFunction;
pub use streaming_tool_call_tracker::TypeValidation;

// JSON utilities
pub use json::is_parsable_json;

// Version constant
pub use version::VERSION;

// Uint8 utilities
pub use uint8_utils::convert_base64_to_bytes;
pub use uint8_utils::convert_bytes_to_base64;
pub use uint8_utils::convert_to_base64;
