//! Language model V4 source reference type.
//!
//! A source that has been used as input to generate the response.

use crate::shared::ProviderMetadata;
use serde::Deserialize;
use serde::Serialize;

/// A source that has been used as input to generate the response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "sourceType", rename_all = "lowercase")]
pub enum LanguageModelV4Source {
    /// URL source referencing web content.
    Url {
        /// The ID of the source.
        id: String,
        /// The URL of the source.
        url: String,
        /// The title of the source.
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// Provider-specific metadata.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// Document source referencing files/documents.
    Document {
        /// The ID of the source.
        id: String,
        /// IANA media type of the document (e.g., 'application/pdf').
        #[serde(rename = "mediaType")]
        media_type: String,
        /// The title of the document.
        title: String,
        /// Optional filename of the document.
        #[serde(skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        /// Provider-specific metadata.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
}

impl LanguageModelV4Source {
    /// Create a URL source.
    pub fn url(id: impl Into<String>, url: impl Into<String>) -> Self {
        Self::Url {
            id: id.into(),
            url: url.into(),
            title: None,
            provider_metadata: None,
        }
    }

    /// Create a document source.
    pub fn document(
        id: impl Into<String>,
        media_type: impl Into<String>,
        title: impl Into<String>,
    ) -> Self {
        Self::Document {
            id: id.into(),
            media_type: media_type.into(),
            title: title.into(),
            filename: None,
            provider_metadata: None,
        }
    }

    /// Add a title to a URL source.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        match &mut self {
            Self::Url { title: t, .. } => *t = Some(title.into()),
            Self::Document { .. } => {}
        }
        self
    }

    /// Add provider metadata.
    pub fn with_metadata(mut self, metadata: ProviderMetadata) -> Self {
        match &mut self {
            Self::Url {
                provider_metadata: pm,
                ..
            } => *pm = Some(metadata),
            Self::Document {
                provider_metadata: pm,
                ..
            } => *pm = Some(metadata),
        }
        self
    }
}

/// Types of sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceType {
    /// URL source referencing web content.
    Url,
    /// Document source referencing files/documents.
    Document,
}

#[cfg(test)]
#[path = "source.test.rs"]
mod tests;
