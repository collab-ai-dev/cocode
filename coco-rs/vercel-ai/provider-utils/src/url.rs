//! URL support utilities.

use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

/// Check if a URL is supported by a model.
///
/// A URL is supported if it matches any of the patterns for the given scheme.
pub fn is_url_supported(url: &str, supported_urls: &HashMap<String, Vec<Regex>>) -> bool {
    // Extract the scheme
    let scheme = url.split(':').next().unwrap_or("");
    let scheme = scheme.to_lowercase();

    if let Some(patterns) = supported_urls.get(&scheme) {
        for pattern in patterns {
            if pattern.is_match(url) {
                return true;
            }
        }
    }

    false
}

/// Parse a data URL into its components.
///
/// Format: `data:[<mediatype>][;base64],<data>`
pub fn parse_data_url(url: &str) -> Option<DataUrl> {
    if !url.starts_with("data:") {
        return None;
    }

    let url = &url[5..]; // Remove "data:"
    let comma_pos = url.find(',')?;
    let (metadata, data) = url.split_at(comma_pos);
    let data = &data[1..]; // Remove the comma

    let (media_type, is_base64) = if let Some(semicolon_pos) = metadata.find(';') {
        let (media_type, encoding) = metadata.split_at(semicolon_pos);
        let encoding = &encoding[1..]; // Remove the semicolon
        let is_base64 = encoding == "base64";
        (Some(media_type.to_string()), is_base64)
    } else if metadata.is_empty() {
        (Some("text/plain".to_string()), false)
    } else {
        (Some(metadata.to_string()), false)
    };

    Some(DataUrl {
        media_type,
        is_base64,
        data: data.to_string(),
    })
}

/// A parsed data URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataUrl {
    /// The media type (MIME type).
    pub media_type: Option<String>,
    /// Whether the data is base64-encoded.
    pub is_base64: bool,
    /// The data portion of the URL.
    pub data: String,
}

impl DataUrl {
    /// Decode the data as bytes.
    pub fn decode(&self) -> Option<Vec<u8>> {
        if self.is_base64 {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .decode(&self.data)
                .ok()
        } else {
            // URL decode
            Some(urlencoding_decode(&self.data))
        }
    }
}

/// Simple URL decoding (handles %XX sequences).
fn urlencoding_decode(s: &str) -> Vec<u8> {
    let mut result = Vec::new();
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                result.push(byte);
            } else {
                result.extend_from_slice(b"%");
                result.extend_from_slice(hex.as_bytes());
            }
        } else {
            result.push(c as u8);
        }
    }

    result
}

// Static regex patterns for image URLs
// These patterns are known to be valid at compile time
#[allow(clippy::unwrap_used)]
static HTTP_IMAGE_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^https?://.*\.(jpg|jpeg|png|gif|webp|bmp)(\?.*)?$").unwrap());
#[allow(clippy::unwrap_used)]
static HTTP_SVG_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^https?://.*\.(svg)(\?.*)?$").unwrap());
#[allow(clippy::unwrap_used)]
static DATA_IMAGE_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^data:image/[^;]+;(base64,)?").unwrap());

/// Common URL patterns for image support.
pub fn image_url_patterns() -> HashMap<String, Vec<Regex>> {
    let mut patterns = HashMap::new();

    // HTTP/HTTPS patterns for common image hosting
    let http_patterns = vec![HTTP_IMAGE_PATTERN.clone(), HTTP_SVG_PATTERN.clone()];
    patterns.insert("http".to_string(), http_patterns.clone());
    patterns.insert("https".to_string(), http_patterns);

    // Data URL pattern for images
    patterns.insert("data".to_string(), vec![DATA_IMAGE_PATTERN.clone()]);

    patterns
}

#[cfg(test)]
#[path = "url.test.rs"]
mod tests;
