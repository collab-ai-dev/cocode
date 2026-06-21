//! Data content types for binary data handling.
//!
//! DataContent represents binary data that can be in multiple formats:
//! - Uint8Array (bytes)
//! - Base64-encoded string
//! - URL

use serde::Deserialize;
use serde::Serialize;

/// Content that represents binary data.
///
/// This can be raw bytes, a base64-encoded string, or a URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataContent {
    /// Raw bytes.
    Bytes(Vec<u8>),
    /// Base64-encoded string.
    Base64(String),
    /// URL to the data.
    Url(String),
}

impl DataContent {
    /// Create DataContent from raw bytes.
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self::Bytes(bytes)
    }

    /// Create DataContent from a base64-encoded string.
    pub fn from_base64(base64: impl Into<String>) -> Self {
        Self::Base64(base64.into())
    }

    /// Create DataContent from a URL.
    pub fn from_url(url: impl Into<String>) -> Self {
        Self::Url(url.into())
    }

    /// Get the data as bytes, decoding if necessary.
    pub fn to_bytes(&self) -> Option<Vec<u8>> {
        match self {
            Self::Bytes(bytes) => Some(bytes.clone()),
            Self::Base64(base64) => base64_decode(base64),
            Self::Url(_) => None, // URL needs to be fetched
        }
    }

    /// Get the data as a base64 string, encoding if necessary.
    pub fn to_base64(&self) -> String {
        match self {
            Self::Bytes(bytes) => base64_encode(bytes),
            Self::Base64(base64) => base64.clone(),
            Self::Url(_) => String::new(), // URL needs to be fetched
        }
    }

    /// Check if this is a URL.
    pub fn is_url(&self) -> bool {
        matches!(self, Self::Url(_))
    }

    /// Get the URL if this is a URL.
    pub fn as_url(&self) -> Option<&str> {
        match self {
            Self::Url(url) => Some(url),
            _ => None,
        }
    }
}

/// Encode bytes to base64.
fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Decode base64 to bytes.
fn base64_decode(base64: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(base64)
        .ok()
}

// Custom serialization for DataContent
impl Serialize for DataContent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Bytes(bytes) => {
                // Serialize as base64 string
                serializer.serialize_str(&base64_encode(bytes))
            }
            Self::Base64(base64) => serializer.serialize_str(base64),
            Self::Url(url) => serializer.serialize_str(url),
        }
    }
}

impl<'de> Deserialize<'de> for DataContent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        // Try to detect if it's a URL
        if s.starts_with("http://") || s.starts_with("https://") || s.starts_with("data:") {
            Ok(Self::Url(s))
        } else {
            // Assume base64
            Ok(Self::Base64(s))
        }
    }
}

/// Media type information for data content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaType {
    /// The MIME type (e.g., "image/png").
    pub mime_type: String,
}

impl MediaType {
    /// Create a new MediaType.
    pub fn new(mime_type: impl Into<String>) -> Self {
        Self {
            mime_type: mime_type.into(),
        }
    }

    /// Check if this is an image type.
    pub fn is_image(&self) -> bool {
        self.mime_type.starts_with("image/")
    }

    /// Check if this is an audio type.
    pub fn is_audio(&self) -> bool {
        self.mime_type.starts_with("audio/")
    }

    /// Check if this is a video type.
    pub fn is_video(&self) -> bool {
        self.mime_type.starts_with("video/")
    }

    /// Check if this is a text type.
    pub fn is_text(&self) -> bool {
        self.mime_type.starts_with("text/")
    }
}

impl From<&str> for MediaType {
    fn from(mime_type: &str) -> Self {
        Self::new(mime_type)
    }
}

impl From<String> for MediaType {
    fn from(mime_type: String) -> Self {
        Self::new(mime_type)
    }
}
