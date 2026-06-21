//! API key loading utilities.

use std::env;
use vercel_ai_provider::LoadAPIKeyError;

/// Load an API key from environment variables.
///
/// # Arguments
///
/// * `api_key` - Optional API key. If provided, this is returned directly.
/// * `env_var` - The environment variable name to check.
/// * `description` - A description of the provider for error messages.
///
/// # Returns
///
/// The API key if found, or a `LoadAPIKeyError`.
///
/// # Example
///
/// ```ignore
/// let api_key = load_api_key(None, "OPENAI_API_KEY", "OpenAI")?;
/// let api_key = load_api_key(Some("sk-xxx".to_string()), "OPENAI_API_KEY", "OpenAI")?;
/// ```
pub fn load_api_key(
    api_key: Option<&str>,
    env_var: &str,
    _description: &str,
) -> Result<String, LoadAPIKeyError> {
    // If API key is provided directly, use it
    if let Some(key) = api_key
        && !key.is_empty()
    {
        return Ok(key.to_string());
    }

    // Try to load from environment variable
    env::var(env_var).map_err(|_| LoadAPIKeyError::missing_env_var(env_var))
}

/// Load an optional API key from environment variables.
///
/// Returns `None` if the API key is not found.
pub fn load_optional_api_key(api_key: Option<&str>, env_var: &str) -> Option<String> {
    // If API key is provided directly, use it
    if let Some(key) = api_key
        && !key.is_empty()
    {
        return Some(key.to_string());
    }

    // Try to load from environment variable
    env::var(env_var).ok()
}

#[cfg(test)]
#[path = "load_api_key.test.rs"]
mod tests;
