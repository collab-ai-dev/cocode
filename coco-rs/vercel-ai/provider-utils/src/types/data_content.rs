//! Data content type.

use serde::Deserialize;
use serde::Serialize;

/// Data content.
///
/// Can either be a base64-encoded string, bytes, or a URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DataContent {
    /// Base64-encoded string.
    Base64(String),
    /// URL to the data.
    Url(String),
    /// Raw bytes.
    #[serde(skip)]
    Bytes(Vec<u8>),
}

impl DataContent {
    /// Create from base64 string.
    pub fn from_base64(base64: impl Into<String>) -> Self {
        Self::Base64(base64.into())
    }

    /// Create from URL.
    pub fn from_url(url: impl Into<String>) -> Self {
        Self::Url(url.into())
    }

    /// Create from bytes.
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self::Bytes(bytes)
    }

    /// Get as base64 string if available.
    pub fn as_base64(&self) -> Option<&str> {
        match self {
            Self::Base64(s) => Some(s),
            _ => None,
        }
    }

    /// Get as URL if available.
    pub fn as_url(&self) -> Option<&str> {
        match self {
            Self::Url(s) => Some(s),
            _ => None,
        }
    }

    /// Get as bytes if available.
    pub fn as_bytes(&self) -> Option<&Vec<u8>> {
        match self {
            Self::Bytes(b) => Some(b),
            _ => None,
        }
    }

    /// Check if this is base64.
    pub fn is_base64(&self) -> bool {
        matches!(self, Self::Base64(_))
    }

    /// Check if this is a URL.
    pub fn is_url(&self) -> bool {
        matches!(self, Self::Url(_))
    }

    /// Check if this is bytes.
    pub fn is_bytes(&self) -> bool {
        matches!(self, Self::Bytes(_))
    }
}

impl From<String> for DataContent {
    fn from(s: String) -> Self {
        Self::Base64(s)
    }
}

impl From<&str> for DataContent {
    fn from(s: &str) -> Self {
        Self::Base64(s.to_string())
    }
}

impl From<Vec<u8>> for DataContent {
    fn from(bytes: Vec<u8>) -> Self {
        Self::Bytes(bytes)
    }
}
