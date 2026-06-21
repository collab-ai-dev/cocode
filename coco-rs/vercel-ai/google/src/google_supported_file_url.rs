//! Utility to check if a URL is a supported Google file URL.

/// Check if a URL is a supported file URL for Google's API.
///
/// Supports:
/// - Google Files API URLs (generativelanguage.googleapis.com/v1beta/files/)
/// - YouTube URLs (youtube.com/watch, youtu.be)
pub fn is_supported_file_url(url: &str) -> bool {
    url.contains("generativelanguage.googleapis.com/v1beta/files/")
        || url.contains("youtube.com/watch")
        || url.contains("youtu.be/")
}

#[cfg(test)]
#[path = "google_supported_file_url.test.rs"]
mod tests;
