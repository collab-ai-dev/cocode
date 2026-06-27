//! Data URI parsing and encoding utilities.
//!
//! This module provides utilities for working with data URIs (RFC 2397).

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use std::borrow::Cow;

/// A parsed data URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataUri {
    /// The MIME type (e.g., "image/png").
    pub media_type: String,
    /// The decoded data.
    pub data: Vec<u8>,
    /// Whether the data was base64 encoded.
    pub base64_encoded: bool,
}

impl DataUri {
    /// Create a new data URI.
    pub fn new(media_type: impl Into<String>, data: Vec<u8>) -> Self {
        Self {
            media_type: media_type.into(),
            data,
            base64_encoded: false,
        }
    }

    /// Create a base64-encoded data URI.
    pub fn new_base64(media_type: impl Into<String>, data: Vec<u8>) -> Self {
        Self {
            media_type: media_type.into(),
            data,
            base64_encoded: true,
        }
    }

    /// Convert to a data URI string.
    pub fn to_uri(&self) -> String {
        if self.base64_encoded {
            let encoded = BASE64.encode(&self.data);
            format!("data:{};base64,{}", self.media_type, encoded)
        } else {
            // URL-encode the data as UTF-8 string if possible
            match std::str::from_utf8(&self.data) {
                Ok(s) => {
                    let encoded = urlencoding::encode(s);
                    format!("data:{},{}", self.media_type, encoded)
                }
                Err(_) => {
                    // Fall back to base64 for non-UTF8 data
                    let encoded = BASE64.encode(&self.data);
                    format!("data:{};base64,{}", self.media_type, encoded)
                }
            }
        }
    }
}

/// Parse a data URI string.
///
/// Format: `data:[<mediatype>][;base64],<data>`
///
/// # Example
///
/// ```
/// use vercel_ai_provider_utils::parse_data_uri;
///
/// let uri = parse_data_uri("data:text/plain,Hello%20World").unwrap();
/// assert_eq!(uri.media_type, "text/plain");
/// assert_eq!(uri.data, b"Hello World");
///
/// let uri = parse_data_uri("data:image/png;base64,iVBORw0KGgo=").unwrap();
/// assert_eq!(uri.media_type, "image/png");
/// assert!(uri.base64_encoded);
/// ```
pub fn parse_data_uri(uri: &str) -> Option<DataUri> {
    const DATA_PREFIX: &str = "data:";

    if !uri.starts_with(DATA_PREFIX) {
        return None;
    }

    let rest = &uri[DATA_PREFIX.len()..];

    // Find the comma separating metadata from data
    let comma_pos = rest.find(',')?;
    let metadata = &rest[..comma_pos];
    let data_str = &rest[comma_pos + 1..];

    // Parse metadata: [<mediatype>][;base64]
    let (media_type, base64_encoded) = if metadata.is_empty() {
        ("text/plain".to_string(), false)
    } else if let Some(stripped) = metadata.strip_suffix(";base64") {
        let media_type = stripped.to_string();
        if media_type.is_empty() {
            ("text/plain".to_string(), true)
        } else {
            (media_type, true)
        }
    } else {
        (metadata.to_string(), false)
    };

    // Decode the data
    let data = if base64_encoded {
        BASE64.decode(data_str).ok()?
    } else {
        // URL decode
        let decoded: Cow<str> = urlencoding::decode(data_str).ok()?;
        decoded.as_bytes().to_vec()
    };

    Some(DataUri {
        media_type,
        data,
        base64_encoded,
    })
}

/// Encode data as a base64 data URI.
pub fn encode_data_uri(media_type: &str, data: &[u8]) -> String {
    let encoded = BASE64.encode(data);
    format!("data:{media_type};base64,{encoded}")
}

/// Encode a string as a data URI.
pub fn encode_text_uri(media_type: &str, text: &str) -> String {
    let encoded = urlencoding::encode(text);
    format!("data:{media_type},{encoded}")
}

#[cfg(test)]
#[path = "data_uri.test.rs"]
mod tests;
