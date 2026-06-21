//! Message conversion errors.
//!
//! This module provides error types for message conversion operations.

use thiserror::Error;

/// Error thrown when message conversion fails.
#[derive(Debug, Error)]
pub enum MessageConversionError {
    /// Invalid message role.
    #[error("Invalid message role: {0}")]
    InvalidRole(String),

    /// Missing required field.
    #[error("Missing required field: {0}")]
    MissingField(String),

    /// Invalid content type.
    #[error("Invalid content type for message: {0}")]
    InvalidContentType(String),

    /// Tool call ID mismatch.
    #[error("Tool call ID mismatch: expected {expected}, got {actual}")]
    ToolCallIdMismatch {
        /// Expected tool call ID.
        expected: String,
        /// Actual tool call ID.
        actual: String,
    },

    /// Unsupported message type.
    #[error("Unsupported message type: {0}")]
    UnsupportedType(String),

    /// JSON parsing error.
    #[error("JSON parsing error: {0}")]
    JsonError(#[from] serde_json::Error),

    /// Base64 decoding error.
    #[error("Base64 decoding error: {0}")]
    Base64Error(String),

    /// Invalid image data.
    #[error("Invalid image data: {0}")]
    InvalidImageData(String),

    /// Invalid file data.
    #[error("Invalid file data: {0}")]
    InvalidFileData(String),
}

impl MessageConversionError {
    /// Create an invalid role error.
    pub fn invalid_role(role: impl Into<String>) -> Self {
        Self::InvalidRole(role.into())
    }

    /// Create a missing field error.
    pub fn missing_field(field: impl Into<String>) -> Self {
        Self::MissingField(field.into())
    }

    /// Create an invalid content type error.
    pub fn invalid_content_type(content_type: impl Into<String>) -> Self {
        Self::InvalidContentType(content_type.into())
    }

    /// Create a tool call ID mismatch error.
    pub fn tool_call_id_mismatch(expected: impl Into<String>, actual: impl Into<String>) -> Self {
        Self::ToolCallIdMismatch {
            expected: expected.into(),
            actual: actual.into(),
        }
    }

    /// Create an unsupported type error.
    pub fn unsupported_type(type_name: impl Into<String>) -> Self {
        Self::UnsupportedType(type_name.into())
    }

    /// Create a base64 error.
    pub fn base64_error(message: impl Into<String>) -> Self {
        Self::Base64Error(message.into())
    }

    /// Create an invalid image data error.
    pub fn invalid_image_data(message: impl Into<String>) -> Self {
        Self::InvalidImageData(message.into())
    }

    /// Create an invalid file data error.
    pub fn invalid_file_data(message: impl Into<String>) -> Self {
        Self::InvalidFileData(message.into())
    }

    /// Check if this is an invalid role error.
    pub fn is_invalid_role(&self) -> bool {
        matches!(self, Self::InvalidRole(_))
    }

    /// Check if this is a missing field error.
    pub fn is_missing_field(&self) -> bool {
        matches!(self, Self::MissingField(_))
    }

    /// Check if this is an unsupported type error.
    pub fn is_unsupported_type(&self) -> bool {
        matches!(self, Self::UnsupportedType(_))
    }
}

/// Result type for message conversion operations.
pub type MessageConversionResult<T> = Result<T, MessageConversionError>;
