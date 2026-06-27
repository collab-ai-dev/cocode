//! Validate download URLs.
//!
//! This module provides utilities for validating URLs used for downloading files.

use url::Url;

/// Validate a download URL.
///
/// Checks that a URL is valid for downloading:
/// - Has a valid URL structure
/// - Uses HTTP or HTTPS protocol
/// - Has a host
///
/// # Examples
///
/// ```
/// use vercel_ai_provider_utils::{validate_download_url, DownloadUrlError};
///
/// let result = validate_download_url("https://example.com/file.zip");
/// assert!(result.is_ok());
///
/// let result = validate_download_url("ftp://example.com/file.zip");
/// assert!(matches!(result, Err(DownloadUrlError::InvalidProtocol)));
/// ```
pub fn validate_download_url(url: &str) -> Result<Url, DownloadUrlError> {
    let parsed = Url::parse(url).map_err(|_| DownloadUrlError::InvalidUrl)?;

    // data: URLs are inline content, not network fetches — always valid.
    if parsed.scheme() == "data" {
        return Ok(parsed);
    }

    // Check protocol
    match parsed.scheme() {
        "http" | "https" => {}
        _ => return Err(DownloadUrlError::InvalidProtocol),
    }

    // Check host
    if parsed.host_str().is_none() {
        return Err(DownloadUrlError::MissingHost);
    }

    Ok(parsed)
}

/// Check if a URL is valid for downloading.
///
/// Returns `true` if the URL is valid for downloading, `false` otherwise.
///
/// # Examples
///
/// ```
/// use vercel_ai_provider_utils::is_valid_download_url;
///
/// assert!(is_valid_download_url("https://example.com/file.zip"));
/// assert!(!is_valid_download_url("ftp://example.com/file.zip"));
/// assert!(!is_valid_download_url("not-a-url"));
/// ```
pub fn is_valid_download_url(url: &str) -> bool {
    validate_download_url(url).is_ok()
}

/// Errors that can occur when validating a download URL.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DownloadUrlError {
    /// The URL is not a valid URL.
    #[error("Invalid URL format")]
    InvalidUrl,

    /// The URL uses an unsupported protocol.
    #[error("Invalid protocol: only HTTP, HTTPS, and data URLs are supported")]
    InvalidProtocol,

    /// The URL is missing a host.
    #[error("URL is missing a host")]
    MissingHost,
}

#[cfg(test)]
#[path = "validate_download_url.test.rs"]
mod tests;
