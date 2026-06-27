//! Language model V4 data content type.
//!
//! Data content can be a Uint8Array, base64 encoded data as a string, or a URL.

use serde::Deserialize;
use serde::Serialize;

/// Data content for language model v4.
///
/// Can be binary data, base64 encoded data as a string, or a URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LanguageModelV4DataContent {
    /// Binary data.
    Bytes(Vec<u8>),
    /// Base64 encoded data.
    Base64(String),
    /// URL reference.
    Url(String),
}

impl LanguageModelV4DataContent {
    /// Create from bytes.
    pub fn bytes(data: Vec<u8>) -> Self {
        Self::Bytes(data)
    }

    /// Create from base64 encoded string.
    pub fn base64(data: impl Into<String>) -> Self {
        Self::Base64(data.into())
    }

    /// Create from URL.
    pub fn url(url: impl Into<String>) -> Self {
        Self::Url(url.into())
    }

    /// Check if this is bytes.
    pub fn is_bytes(&self) -> bool {
        matches!(self, Self::Bytes(_))
    }

    /// Check if this is base64.
    pub fn is_base64(&self) -> bool {
        matches!(self, Self::Base64(_))
    }

    /// Check if this is a URL.
    pub fn is_url(&self) -> bool {
        matches!(self, Self::Url(_))
    }

    /// Get as bytes if available.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes(b) => Some(b),
            _ => None,
        }
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
}

impl Serialize for LanguageModelV4DataContent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Bytes(b) => {
                // Serialize bytes as base64
                use base64::Engine as _;
                use base64::engine::general_purpose::STANDARD;
                serializer.serialize_str(&STANDARD.encode(b))
            }
            Self::Base64(s) => serializer.serialize_str(s),
            Self::Url(s) => serializer.serialize_str(s),
        }
    }
}

impl<'de> Deserialize<'de> for LanguageModelV4DataContent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        // Try to determine if it's a URL or base64 data
        if s.starts_with("http://") || s.starts_with("https://") || s.starts_with("data:") {
            Ok(Self::Url(s))
        } else {
            Ok(Self::Base64(s))
        }
    }
}

// Wire shape is always a string (base64 or URL). Match that in the
// schema rather than letting schemars infer a tagged-enum shape.
// `inline_schema = true` matches schemars 0.8 behavior: parents inline
// the `{"type": "string"}` shape instead of `$ref`-ing a named alias
// for what is already-a-string on the wire.
#[cfg(feature = "schema")]
impl schemars::JsonSchema for LanguageModelV4DataContent {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "LanguageModelV4DataContent".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <String as schemars::JsonSchema>::json_schema(generator)
    }
}

impl From<Vec<u8>> for LanguageModelV4DataContent {
    fn from(data: Vec<u8>) -> Self {
        Self::bytes(data)
    }
}

impl From<&[u8]> for LanguageModelV4DataContent {
    fn from(data: &[u8]) -> Self {
        Self::bytes(data.to_vec())
    }
}

impl From<String> for LanguageModelV4DataContent {
    fn from(s: String) -> Self {
        if s.starts_with("http://") || s.starts_with("https://") || s.starts_with("data:") {
            Self::url(s)
        } else {
            Self::base64(s)
        }
    }
}

impl From<&str> for LanguageModelV4DataContent {
    fn from(s: &str) -> Self {
        s.to_string().into()
    }
}

#[cfg(test)]
#[path = "data_content.test.rs"]
mod tests;
