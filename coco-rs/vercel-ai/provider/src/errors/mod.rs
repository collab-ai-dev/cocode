//! Error types for the Vercel AI SDK.

mod ai_sdk_error;
mod api_call_error;
mod empty_response_body_error;
mod get_error_message;
mod invalid_argument_error;
mod invalid_prompt_error;
mod invalid_response_data_error;
mod json_parse_error;
mod load_api_key_error;
mod load_setting_error;
mod no_content_generated_error;
mod no_such_model_error;
mod no_such_provider_reference_error;
pub mod provider_error;
mod too_many_embedding_values_for_call_error;
mod type_validation_error;
mod unsupported_functionality_error;

pub use ai_sdk_error::AISdkError;
pub use api_call_error::APICallError;
pub use empty_response_body_error::EmptyResponseBodyError;
pub use get_error_message::get_error_message;
pub use invalid_argument_error::InvalidArgumentError;
pub use invalid_prompt_error::InvalidPromptError;
pub use invalid_response_data_error::InvalidResponseDataError;
pub use json_parse_error::JSONParseError;
pub use load_api_key_error::LoadAPIKeyError;
pub use load_setting_error::LoadSettingError;
pub use no_content_generated_error::NoContentGeneratedError;
pub use no_such_model_error::NoSuchModelError;
pub use no_such_provider_reference_error::NoSuchProviderReferenceError;
pub use provider_error::ProviderError;
pub use too_many_embedding_values_for_call_error::TooManyEmbeddingValuesForCallError;
pub use type_validation_error::TypeValidationContext;
pub use type_validation_error::TypeValidationError;
pub use unsupported_functionality_error::UnsupportedFunctionalityError;
