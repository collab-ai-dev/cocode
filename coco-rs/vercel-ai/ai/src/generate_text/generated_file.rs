//! Generated file type.
//!
//! This module provides the `GeneratedFile` type for representing files
//! generated during text generation (e.g., code files, downloads).

use std::path::PathBuf;

/// A file generated during text generation.
#[derive(Debug, Clone)]
pub struct GeneratedFile {
    /// The file name.
    pub name: String,
    /// The file content (base64 encoded for binary files).
    pub content: String,
    /// The media type (MIME type).
    pub media_type: String,
    /// The file path (if saved to disk).
    pub path: Option<PathBuf>,
    /// Whether the content is base64 encoded.
    pub is_base64: bool,
}

impl GeneratedFile {
    /// Create a new generated file.
    pub fn new(
        name: impl Into<String>,
        content: impl Into<String>,
        media_type: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            content: content.into(),
            media_type: media_type.into(),
            path: None,
            is_base64: false,
        }
    }

    /// Create a generated file from base64 content.
    pub fn from_base64(
        name: impl Into<String>,
        base64_content: impl Into<String>,
        media_type: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            content: base64_content.into(),
            media_type: media_type.into(),
            path: None,
            is_base64: true,
        }
    }

    /// Create a text file.
    pub fn text(name: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            content: content.into(),
            media_type: "text/plain".to_string(),
            path: None,
            is_base64: false,
        }
    }

    /// Create a JSON file.
    pub fn json(name: impl Into<String>, content: &serde_json::Value) -> Self {
        Self {
            name: name.into(),
            content: serde_json::to_string_pretty(content).unwrap_or_default(),
            media_type: "application/json".to_string(),
            path: None,
            is_base64: false,
        }
    }

    /// Create a markdown file.
    pub fn markdown(name: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            content: content.into(),
            media_type: "text/markdown".to_string(),
            path: None,
            is_base64: false,
        }
    }

    /// Create a code file with the given language.
    pub fn code(
        name: impl Into<String>,
        content: impl Into<String>,
        language: impl Into<String>,
    ) -> Self {
        let media_type = match language.into().as_str() {
            "rust" => "text/x-rust",
            "python" => "text/x-python",
            "javascript" | "js" => "text/javascript",
            "typescript" | "ts" => "text/typescript",
            "java" => "text/x-java",
            "c" => "text/x-c",
            "cpp" | "c++" => "text/x-c++",
            "go" => "text/x-go",
            "ruby" => "text/x-ruby",
            "php" => "text/x-php",
            "swift" => "text/x-swift",
            "kotlin" => "text/x-kotlin",
            "html" => "text/html",
            "css" => "text/css",
            "sql" => "application/sql",
            "shell" | "bash" => "text/x-sh",
            _ => "text/plain",
        };

        Self {
            name: name.into(),
            content: content.into(),
            media_type: media_type.to_string(),
            path: None,
            is_base64: false,
        }
    }

    /// Set the file path.
    pub fn with_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Get the file extension from the name.
    pub fn extension(&self) -> Option<&str> {
        self.name.rsplit('.').next()
    }

    /// Check if this is a text file.
    pub fn is_text(&self) -> bool {
        self.media_type.starts_with("text/")
            || self.media_type == "application/json"
            || self.media_type == "application/xml"
    }

    /// Check if this is a binary file.
    pub fn is_binary(&self) -> bool {
        !self.is_text()
    }

    /// Get the content as bytes.
    pub fn content_bytes(&self) -> Vec<u8> {
        if self.is_base64 {
            // Decode base64
            use base64::Engine as _;
            use base64::engine::general_purpose::STANDARD;
            STANDARD.decode(&self.content).unwrap_or_default()
        } else {
            self.content.as_bytes().to_vec()
        }
    }

    /// Get the content as text (if it's a text file).
    pub fn content_text(&self) -> Option<&str> {
        if self.is_text() && !self.is_base64 {
            Some(&self.content)
        } else {
            None
        }
    }

    /// Get the file size in bytes.
    pub fn size(&self) -> usize {
        if self.is_base64 {
            // Base64 encoding is ~4/3 the size of original data
            (self.content.len() * 3) / 4
        } else {
            self.content.len()
        }
    }
}

impl PartialEq for GeneratedFile {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.content == other.content
            && self.media_type == other.media_type
    }
}

impl Eq for GeneratedFile {}

/// A collection of generated files.
#[derive(Debug, Clone, Default)]
pub struct GeneratedFiles {
    /// The files.
    pub files: Vec<GeneratedFile>,
}

impl GeneratedFiles {
    /// Create a new collection.
    pub fn new() -> Self {
        Self { files: Vec::new() }
    }

    /// Add a file.
    pub fn add(&mut self, file: GeneratedFile) {
        self.files.push(file);
    }

    /// Get a file by name.
    pub fn get(&self, name: &str) -> Option<&GeneratedFile> {
        self.files.iter().find(|f| f.name == name)
    }

    /// Check if a file exists.
    pub fn contains(&self, name: &str) -> bool {
        self.files.iter().any(|f| f.name == name)
    }

    /// Get the number of files.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Check if there are no files.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Filter files by media type.
    pub fn filter_by_media_type(&self, media_type: &str) -> Vec<&GeneratedFile> {
        self.files
            .iter()
            .filter(|f| f.media_type == media_type)
            .collect()
    }

    /// Get all text files.
    pub fn text_files(&self) -> Vec<&GeneratedFile> {
        self.files.iter().filter(|f| f.is_text()).collect()
    }

    /// Get all binary files.
    pub fn binary_files(&self) -> Vec<&GeneratedFile> {
        self.files.iter().filter(|f| f.is_binary()).collect()
    }

    /// Get the total size of all files.
    pub fn total_size(&self) -> usize {
        self.files.iter().map(GeneratedFile::size).sum()
    }
}

impl From<Vec<GeneratedFile>> for GeneratedFiles {
    fn from(files: Vec<GeneratedFile>) -> Self {
        Self { files }
    }
}
