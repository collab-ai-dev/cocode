//! Tagged file data types for the v4 provider spec.

use std::collections::HashMap;

use serde::Deserialize;
use serde::Serialize;

/// Raw file data: either binary bytes or a base64-encoded string.
///
/// Either raw binary bytes or a base64-encoded string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileRawData {
    /// Raw binary bytes. Serialized as base64 in JSON.
    Bytes(Vec<u8>),
    /// Base64-encoded string.
    Base64(String),
}

impl FileRawData {
    /// Create from bytes.
    pub fn bytes(data: Vec<u8>) -> Self {
        Self::Bytes(data)
    }

    /// Create from a base64-encoded string.
    pub fn base64(data: impl Into<String>) -> Self {
        Self::Base64(data.into())
    }

    /// Return the bytes, decoding from base64 if necessary.
    pub fn to_bytes(&self) -> Option<Vec<u8>> {
        match self {
            Self::Bytes(b) => Some(b.clone()),
            Self::Base64(s) => base64_decode(s),
        }
    }

    /// Return the base64 string, encoding from bytes if necessary.
    pub fn to_base64(&self) -> String {
        match self {
            Self::Bytes(b) => base64_encode(b),
            Self::Base64(s) => s.clone(),
        }
    }

    /// Return a reference to the raw bytes if this is the `Bytes` variant.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes(b) => Some(b),
            _ => None,
        }
    }

    /// Return the base64 string if this is the `Base64` variant.
    pub fn as_base64(&self) -> Option<&str> {
        match self {
            Self::Base64(s) => Some(s),
            _ => None,
        }
    }
}

impl Serialize for FileRawData {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_base64())
    }
}

impl<'de> Deserialize<'de> for FileRawData {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(Self::Base64(s))
    }
}

// Wire shape is always a string (base64). Match that in the schema
// rather than letting schemars infer a tagged-enum shape.
// `inline_schema = true` matches schemars 0.8 behavior: parents inline
// the `{"type": "string"}` shape instead of `$ref`-ing a named alias
// for what is already-a-string on the wire.
#[cfg(feature = "schema")]
impl schemars::JsonSchema for FileRawData {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "FileRawData".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <String as schemars::JsonSchema>::json_schema(generator)
    }
}

impl From<Vec<u8>> for FileRawData {
    fn from(bytes: Vec<u8>) -> Self {
        Self::Bytes(bytes)
    }
}

impl From<String> for FileRawData {
    fn from(s: String) -> Self {
        Self::Base64(s)
    }
}

impl From<&str> for FileRawData {
    fn from(s: &str) -> Self {
        Self::Base64(s.to_string())
    }
}

/// File data as a tagged discriminated union (v4 spec).
///
/// - `Data` — raw bytes or base64-encoded string.
/// - `Url`  — a URL pointing to the file.
/// - `Reference` — a provider reference (`{ [provider]: id }`).
/// - `Text` — inline text content.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SharedV4FileData {
    /// Raw bytes (Uint8Array) or base64-encoded string.
    Data {
        /// The raw data.
        data: FileRawData,
    },
    /// A URL that points to the file.
    Url {
        /// The URL string.
        url: String,
    },
    /// A provider reference mapping provider names to file IDs.
    Reference {
        /// Map of provider name → provider-specific file ID.
        reference: HashMap<String, String>,
    },
    /// Inline text content (e.g. an inline text document).
    Text {
        /// The text content.
        text: String,
    },
}

impl SharedV4FileData {
    /// Create a data variant from raw bytes.
    pub fn data_bytes(bytes: Vec<u8>) -> Self {
        Self::Data {
            data: FileRawData::Bytes(bytes),
        }
    }

    /// Create a data variant from a base64 string.
    pub fn data_base64(base64: impl Into<String>) -> Self {
        Self::Data {
            data: FileRawData::Base64(base64.into()),
        }
    }

    /// Create a URL variant.
    pub fn url(url: impl Into<String>) -> Self {
        Self::Url { url: url.into() }
    }

    /// Create a reference variant.
    pub fn reference(reference: HashMap<String, String>) -> Self {
        Self::Reference { reference }
    }

    /// Create a text variant.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// Return the inner `FileRawData` if this is a `Data` variant.
    pub fn as_data(&self) -> Option<&FileRawData> {
        match self {
            Self::Data { data } => Some(data),
            _ => None,
        }
    }

    /// Return the URL string if this is a `Url` variant.
    pub fn as_url(&self) -> Option<&str> {
        match self {
            Self::Url { url } => Some(url),
            _ => None,
        }
    }

    /// Return the provider reference map if this is a `Reference` variant.
    pub fn as_reference(&self) -> Option<&HashMap<String, String>> {
        match self {
            Self::Reference { reference } => Some(reference),
            _ => None,
        }
    }

    /// Return the text if this is a `Text` variant.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            _ => None,
        }
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode(s).ok()
}
