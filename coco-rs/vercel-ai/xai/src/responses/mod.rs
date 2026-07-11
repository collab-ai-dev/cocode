//! xAI Responses API surface.
//!
//! Opt-in alternative to Chat Completions, reached via `XaiProvider::responses`.
//! The wire format mirrors the OpenAI Responses API (typed `input` items,
//! `output` items, and `response.*` SSE events).

pub mod convert_to_xai_responses_input;
pub mod convert_xai_responses_usage;
pub mod map_xai_responses_finish_reason;
pub mod xai_responses_api_types;
pub mod xai_responses_language_model;
pub mod xai_responses_options;
pub mod xai_responses_prepare_tools;
pub mod xai_responses_stream;
pub mod xai_tools;

pub use convert_to_xai_responses_input::convert_to_xai_responses_input;
pub use convert_xai_responses_usage::XaiResponsesUsage;
pub use convert_xai_responses_usage::convert_xai_responses_usage;
pub use map_xai_responses_finish_reason::map_xai_responses_finish_reason;
pub use xai_responses_language_model::XaiResponsesLanguageModel;
pub use xai_responses_options::XaiResponsesProviderOptions;
pub use xai_responses_options::extract_xai_responses_options;
pub use xai_responses_prepare_tools::PreparedXaiResponsesTools;
pub use xai_responses_prepare_tools::prepare_responses_tools;
