//! Split data URLs into components.
//!
//! This module provides functionality to parse data URLs (RFC 2397) into
//! their media type and base64 content components.

/// The result of splitting a data URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitDataUrl {
    /// The media type (MIME type), if present.
    pub media_type: Option<String>,
    /// The base64-encoded content, if present.
    pub base64_content: Option<String>,
}

impl SplitDataUrl {
    /// Create a new split data URL result.
    pub fn new(media_type: Option<String>, base64_content: Option<String>) -> Self {
        Self {
            media_type,
            base64_content,
        }
    }
}

/// Split a data URL into its components.
///
/// Data URLs have the format: `data:[<mediatype>][;base64],<data>`
///
/// # Arguments
///
/// * `data_url` - The data URL string to split
///
/// # Returns
///
/// A `SplitDataUrl` containing the media type and base64 content.
/// If parsing fails, both fields will be `None`.
///
/// # Example
///
/// ```
/// use vercel_ai::util::split_data_url::split_data_url;
///
/// let result = split_data_url("data:image/png;base64,iVBORw0KGgo=");
/// assert_eq!(result.media_type, Some("image/png".to_string()));
/// assert_eq!(result.base64_content, Some("iVBORw0KGgo=".to_string()));
/// ```
pub fn split_data_url(data_url: &str) -> SplitDataUrl {
    // Check if it starts with "data:"
    if !data_url.starts_with("data:") {
        return SplitDataUrl::new(None, None);
    }

    // Remove "data:" prefix
    let rest = &data_url[5..];

    // Split on the first comma
    let parts: Vec<&str> = rest.splitn(2, ',').collect();
    if parts.len() != 2 {
        return SplitDataUrl::new(None, None);
    }

    let header = parts[0];
    let base64_content = parts[1].to_string();

    // Parse the header for media type
    // Format: [<mediatype>][;base64]
    let header_parts: Vec<&str> = header.split(';').collect();
    let media_type = if header_parts.is_empty() || header_parts[0].is_empty() {
        None
    } else {
        Some(header_parts[0].to_string())
    };

    SplitDataUrl::new(media_type, Some(base64_content))
}

/// Check if a string is a data URL.
pub fn is_data_url(s: &str) -> bool {
    s.starts_with("data:")
}

/// Check if a string is an HTTP or HTTPS URL.
pub fn is_http_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

#[cfg(test)]
#[path = "split_data_url.test.rs"]
mod tests;
