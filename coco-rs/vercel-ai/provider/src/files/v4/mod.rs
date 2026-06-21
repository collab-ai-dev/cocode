//! Files interface V4 types.

use std::collections::HashMap;

use serde::Deserialize;
use serde::Serialize;

use crate::shared::FileRawData;
use crate::shared::ProviderMetadata;
use crate::shared::ProviderOptions;
use crate::shared::Warning;

/// Options for uploading a file via the files interface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesV4UploadFileCallOptions {
    /// The file data (raw bytes/base64 or inline text).
    pub data: FilesV4UploadData,
    /// The IANA media type of the file (e.g. `application/pdf`).
    pub media_type: String,
    /// Optional filename.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    /// Additional provider-specific options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
}

/// Data variants for file upload — either raw bytes/base64 or inline text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum FilesV4UploadData {
    /// Raw bytes (Uint8Array) or base64-encoded string.
    Data {
        /// The raw data.
        data: FileRawData,
    },
    /// Inline text (UTF-8).
    Text {
        /// The text content.
        text: String,
    },
}

impl FilesV4UploadData {
    /// Create from bytes.
    pub fn bytes(bytes: Vec<u8>) -> Self {
        Self::Data {
            data: FileRawData::Bytes(bytes),
        }
    }

    /// Create from a base64 string.
    pub fn base64(base64: impl Into<String>) -> Self {
        Self::Data {
            data: FileRawData::Base64(base64.into()),
        }
    }

    /// Create from text.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }
}

/// Result of uploading a file via the files interface.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesV4UploadFileResult {
    /// Provider reference mapping provider names to provider-specific file IDs.
    pub provider_reference: HashMap<String, String>,
    /// The IANA media type of the uploaded file, if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// The filename of the uploaded file, if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    /// Additional provider-specific metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ProviderMetadata>,
    /// Warnings from the provider.
    pub warnings: Vec<Warning>,
}

/// File management interface — implements the files interface version 4.
#[async_trait::async_trait]
pub trait FilesV4: Send + Sync {
    /// The files interface specification version.
    fn specification_version(&self) -> &'static str {
        "v4"
    }

    /// The provider ID.
    fn provider(&self) -> &str;

    /// Uploads a file to the provider and returns a provider reference.
    async fn upload_file(
        &self,
        options: FilesV4UploadFileCallOptions,
    ) -> Result<FilesV4UploadFileResult, crate::errors::AISdkError>;
}
