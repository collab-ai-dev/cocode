//! Multipart form data construction utilities.
//!
//! This module provides utilities for building multipart/form-data requests.

use reqwest::multipart::Form;
use reqwest::multipart::Part;
use std::path::Path;

/// A builder for multipart form data.
#[derive(Debug, Default)]
pub struct FormData {
    form: Form,
}

impl FormData {
    /// Create a new empty form data builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a text field.
    pub fn text(mut self, name: &str, value: impl Into<String>) -> Self {
        self.form = self.form.text(name.to_string(), value.into());
        self
    }

    /// Add a file from bytes.
    pub fn bytes(mut self, name: &str, bytes: Vec<u8>, filename: &str) -> Self {
        let part = Part::bytes(bytes).file_name(filename.to_string());
        self.form = self.form.part(name.to_string(), part);
        self
    }

    /// Add a file from bytes with a specific MIME type.
    pub fn bytes_with_mime(
        mut self,
        name: &str,
        bytes: Vec<u8>,
        filename: &str,
        mime_type: &str,
    ) -> Self {
        let part = Part::bytes(bytes)
            .file_name(filename.to_string())
            .mime_str(mime_type)
            .unwrap_or_else(|_| Part::bytes(vec![]).file_name(filename.to_string()));
        self.form = self.form.part(name.to_string(), part);
        self
    }

    /// Add a file from a path.
    pub async fn file(self, name: &str, path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref();
        let bytes = tokio::fs::read(path).await?;
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());
        Ok(self.bytes(name, bytes, &filename))
    }

    /// Add a JSON part.
    pub fn json(mut self, name: &str, value: &serde_json::Value) -> Self {
        let bytes = serde_json::to_vec(value).unwrap_or_default();
        let part = Part::bytes(bytes)
            .file_name("data.json")
            .mime_str("application/json")
            .unwrap_or_else(|_| Part::bytes(vec![]).file_name("data.json"));
        self.form = self.form.part(name.to_string(), part);
        self
    }

    /// Build the final multipart form.
    pub fn build(self) -> Form {
        self.form
    }
}

impl From<FormData> for Form {
    fn from(form_data: FormData) -> Self {
        form_data.build()
    }
}

#[cfg(test)]
#[path = "form_data.test.rs"]
mod tests;
