//! Data content handling for prompts.
//!
//! This module provides utilities for working with data content
//! (base64, URLs, binary data) in prompts.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use vercel_ai_provider::AISdkError;

use crate::util::split_data_url::split_data_url;

/// Data content that can be base64 string, URL, or binary data.
#[derive(Debug, Clone)]
pub enum DataContentValue {
    /// Base64-encoded string.
    Base64(String),
    /// URL to the data.
    Url(String),
    /// Binary data.
    Binary(Vec<u8>),
}

impl DataContentValue {
    /// Create from a string (can be base64 or URL).
    pub fn from_string(s: impl Into<String>) -> Self {
        let s = s.into();
        if s.starts_with("http://") || s.starts_with("https://") || s.starts_with("data:") {
            Self::Url(s)
        } else {
            Self::Base64(s)
        }
    }

    /// Create from binary data.
    pub fn from_binary(data: Vec<u8>) -> Self {
        Self::Binary(data)
    }

    /// Convert to base64 string.
    pub fn to_base64(&self) -> Result<String, AISdkError> {
        match self {
            DataContentValue::Base64(s) => Ok(s.clone()),
            DataContentValue::Binary(data) => Ok(BASE64_STANDARD.encode(data)),
            DataContentValue::Url(url) => {
                // For data URLs, extract the base64 content
                if url.starts_with("data:") {
                    let split = split_data_url(url);
                    if let Some(base64_content) = split.base64_content {
                        Ok(base64_content)
                    } else {
                        Err(AISdkError::new(format!("Invalid data URL: {url}")))
                    }
                } else {
                    Err(AISdkError::new(
                        "Cannot convert URL to base64 without downloading",
                    ))
                }
            }
        }
    }

    /// Convert to binary data.
    pub fn to_binary(&self) -> Result<Vec<u8>, AISdkError> {
        match self {
            DataContentValue::Binary(data) => Ok(data.clone()),
            DataContentValue::Base64(s) => BASE64_STANDARD
                .decode(s)
                .map_err(|e| AISdkError::new(format!("Invalid base64 data: {e}"))),
            DataContentValue::Url(url) => {
                // For data URLs, extract and decode the base64 content
                if url.starts_with("data:") {
                    let split = split_data_url(url);
                    if let Some(base64) = split.base64_content {
                        BASE64_STANDARD.decode(&base64).map_err(|e| {
                            AISdkError::new(format!("Invalid base64 in data URL: {e}"))
                        })
                    } else {
                        Err(AISdkError::new(format!("Invalid data URL: {url}")))
                    }
                } else {
                    Err(AISdkError::new(
                        "Cannot convert URL to binary without downloading",
                    ))
                }
            }
        }
    }
}

/// Convert data content to language model v4 data content format.
pub fn convert_to_language_model_data_content(
    content: DataContentValue,
) -> Result<(Vec<u8>, Option<String>), AISdkError> {
    match content {
        DataContentValue::Binary(data) => Ok((data, None)),
        DataContentValue::Base64(s) => {
            let data = BASE64_STANDARD
                .decode(&s)
                .map_err(|e| AISdkError::new(format!("Invalid base64 data: {e}")))?;
            Ok((data, None))
        }
        DataContentValue::Url(url) => {
            // Handle data URLs
            if url.starts_with("data:") {
                let split = split_data_url(&url);
                if let (Some(media_type), Some(base64)) = (split.media_type, split.base64_content) {
                    let data = BASE64_STANDARD
                        .decode(&base64)
                        .map_err(|e| AISdkError::new(format!("Invalid base64 in data URL: {e}")))?;
                    Ok((data, Some(media_type)))
                } else {
                    Err(AISdkError::new(format!("Invalid data URL format: {url}")))
                }
            } else {
                Err(AISdkError::new("URL must be a data URL for inline content"))
            }
        }
    }
}

/// Convert a Uint8Array to text.
pub fn convert_uint8_array_to_text(data: &[u8]) -> Result<String, AISdkError> {
    String::from_utf8(data.to_vec())
        .map_err(|e| AISdkError::new(format!("Cannot convert binary data to text: {e}")))
}

#[cfg(test)]
#[path = "data_content.test.rs"]
mod tests;
