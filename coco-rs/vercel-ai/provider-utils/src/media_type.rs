//! MIME type handling utilities.
//!
//! This module provides utilities for working with MIME types.

use std::collections::HashMap;
use std::sync::LazyLock;

/// Common MIME types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MediaType(pub &'static str);

impl MediaType {
    // Text types
    pub const TEXT_PLAIN: Self = Self("text/plain");
    pub const TEXT_HTML: Self = Self("text/html");
    pub const TEXT_CSS: Self = Self("text/css");
    pub const TEXT_CSV: Self = Self("text/csv");
    pub const TEXT_JAVASCRIPT: Self = Self("text/javascript");

    // JSON types
    pub const JSON: Self = Self("application/json");
    pub const JSONL: Self = Self("application/jsonl");

    // Image types
    pub const IMAGE_PNG: Self = Self("image/png");
    pub const IMAGE_JPEG: Self = Self("image/jpeg");
    pub const IMAGE_GIF: Self = Self("image/gif");
    pub const IMAGE_WEBP: Self = Self("image/webp");
    pub const IMAGE_SVG: Self = Self("image/svg+xml");

    // Audio types
    pub const AUDIO_MP3: Self = Self("audio/mpeg");
    pub const AUDIO_WAV: Self = Self("audio/wav");
    pub const AUDIO_OGG: Self = Self("audio/ogg");
    pub const AUDIO_WEBM: Self = Self("audio/webm");

    // Video types
    pub const VIDEO_MP4: Self = Self("video/mp4");
    pub const VIDEO_WEBM: Self = Self("video/webm");

    // Document types
    pub const PDF: Self = Self("application/pdf");
    pub const ZIP: Self = Self("application/zip");

    // Binary
    pub const OCTET_STREAM: Self = Self("application/octet-stream");

    /// Get the MIME type as a string.
    pub fn as_str(&self) -> &'static str {
        self.0
    }

    /// Check if this is an image type.
    pub fn is_image(&self) -> bool {
        self.0.starts_with("image/")
    }

    /// Check if this is an audio type.
    pub fn is_audio(&self) -> bool {
        self.0.starts_with("audio/")
    }

    /// Check if this is a video type.
    pub fn is_video(&self) -> bool {
        self.0.starts_with("video/")
    }

    /// Check if this is a text type.
    pub fn is_text(&self) -> bool {
        self.0.starts_with("text/") || *self == Self::JSON || *self == Self::JSONL
    }

    /// Get the file extension for this MIME type.
    pub fn extension(&self) -> Option<&'static str> {
        EXTENSION_MAP.get(self.0).copied()
    }
}

impl std::fmt::Display for MediaType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<MediaType> for String {
    fn from(mt: MediaType) -> Self {
        mt.0.to_string()
    }
}

impl From<&MediaType> for String {
    fn from(mt: &MediaType) -> Self {
        mt.0.to_string()
    }
}

/// Map from MIME type to file extension.
static EXTENSION_MAP: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    let mut map = HashMap::new();
    // Text
    map.insert("text/plain", "txt");
    map.insert("text/html", "html");
    map.insert("text/css", "css");
    map.insert("text/csv", "csv");
    map.insert("text/javascript", "js");
    // JSON
    map.insert("application/json", "json");
    map.insert("application/jsonl", "jsonl");
    // Images
    map.insert("image/png", "png");
    map.insert("image/jpeg", "jpg");
    map.insert("image/gif", "gif");
    map.insert("image/webp", "webp");
    map.insert("image/svg+xml", "svg");
    // Audio
    map.insert("audio/mpeg", "mp3");
    map.insert("audio/wav", "wav");
    map.insert("audio/ogg", "ogg");
    map.insert("audio/webm", "weba");
    // Video
    map.insert("video/mp4", "mp4");
    map.insert("video/webm", "webm");
    // Documents
    map.insert("application/pdf", "pdf");
    map.insert("application/zip", "zip");
    map
});

/// Map from file extension to MIME type.
static MIME_MAP: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    let mut map = HashMap::new();
    // Text
    map.insert("txt", "text/plain");
    map.insert("html", "text/html");
    map.insert("htm", "text/html");
    map.insert("css", "text/css");
    map.insert("csv", "text/csv");
    map.insert("js", "text/javascript");
    map.insert("mjs", "text/javascript");
    // JSON
    map.insert("json", "application/json");
    map.insert("jsonl", "application/jsonl");
    // Images
    map.insert("png", "image/png");
    map.insert("jpg", "image/jpeg");
    map.insert("jpeg", "image/jpeg");
    map.insert("gif", "image/gif");
    map.insert("webp", "image/webp");
    map.insert("svg", "image/svg+xml");
    // Audio
    map.insert("mp3", "audio/mpeg");
    map.insert("wav", "audio/wav");
    map.insert("ogg", "audio/ogg");
    map.insert("weba", "audio/webm");
    // Video
    map.insert("mp4", "video/mp4");
    map.insert("webm", "video/webm");
    // Documents
    map.insert("pdf", "application/pdf");
    map.insert("zip", "application/zip");
    map
});

/// Get the MIME type for a file extension.
///
/// # Arguments
///
/// * `extension` - The file extension (without the dot).
///
/// # Returns
///
/// The MIME type, or `application/octet-stream` if unknown.
pub fn media_type_from_extension(extension: &str) -> MediaType {
    MIME_MAP
        .get(extension.to_lowercase().as_str())
        .map(|&s| MediaType(s))
        .unwrap_or(MediaType::OCTET_STREAM)
}

/// Get the MIME type from a filename.
///
/// # Arguments
///
/// * `filename` - The filename (with or without path).
///
/// # Returns
///
/// The MIME type, or `application/octet-stream` if unknown.
pub fn media_type_from_filename(filename: &str) -> MediaType {
    let extension = filename.rsplit('.').next().unwrap_or("");
    media_type_from_extension(extension)
}

/// Parse a MIME type string.
///
/// # Arguments
///
/// * `s` - The MIME type string.
///
/// # Returns
///
/// The MediaType if valid, or `application/octet-stream` if invalid.
pub fn parse_media_type(s: &str) -> MediaType {
    // Basic validation: should contain a slash
    if s.contains('/')
        && s.chars()
            .all(|c| c.is_alphanumeric() || c == '/' || c == '-' || c == '+' || c == '.')
    {
        // Return as a static string if it matches a known type
        for (known, _) in EXTENSION_MAP.iter() {
            if *known == s {
                return MediaType(known);
            }
        }
        // For unknown but valid types, return as octet-stream
        MediaType::OCTET_STREAM
    } else {
        MediaType::OCTET_STREAM
    }
}

/// Returns the top-level segment of a media type (the portion before `/`).
///
/// Examples:
/// - `"image/png"` → `"image"`
/// - `"image/*"` → `"image"`
/// - `"image"` → `"image"`
/// - `"image/"` → `"image"`
/// - `""` → `""`
/// - `"/"` → `""`
pub fn get_top_level_media_type(media_type: &str) -> &str {
    match media_type.find('/') {
        Some(idx) => &media_type[..idx],
        None => media_type,
    }
}

/// Returns `true` only when the given media type has a non-empty, non-wildcard
/// subtype (i.e. matches the form `type/subtype`, and `subtype` is not `*`).
///
/// Examples:
/// - `"image/png"` → `true`
/// - `"image/*"` → `false`
/// - `"image"` → `false`
/// - `"image/"` → `false`
/// - `""` → `false`
/// - `"/"` → `false`
pub fn is_full_media_type(media_type: &str) -> bool {
    match media_type.find('/') {
        Some(idx) => {
            let subtype = &media_type[idx + 1..];
            !subtype.is_empty() && subtype != "*"
        }
        None => false,
    }
}

#[cfg(test)]
#[path = "media_type.test.rs"]
mod tests;
