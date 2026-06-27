//! Download functionality for fetching content from URLs.
//!
//! This module provides async download functionality with size limits and
//! abort signal support.

use reqwest::Client;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

/// Default maximum download size (2 GiB).
pub const DEFAULT_MAX_DOWNLOAD_SIZE: u64 = 2 * 1024 * 1024 * 1024;

/// Download error types.
#[derive(Debug, Error)]
pub enum DownloadError {
    /// HTTP error with status code.
    #[error("Download failed from {url}: HTTP {status_code} {status_text}")]
    Http {
        /// The URL that was being downloaded.
        url: String,
        /// HTTP status code.
        status_code: u16,
        /// HTTP status text.
        status_text: String,
    },

    /// Size limit exceeded.
    #[error("Download from {url} exceeded maximum size of {max_bytes} bytes")]
    SizeLimitExceeded {
        /// The URL that was being downloaded.
        url: String,
        /// Maximum allowed bytes.
        max_bytes: u64,
    },

    /// Network or IO error.
    #[error("Download failed from {url}: {source}")]
    Network {
        /// The URL that was being downloaded.
        url: String,
        /// The underlying error.
        #[source]
        source: reqwest::Error,
    },

    /// Download was cancelled.
    #[error("Download was cancelled")]
    Cancelled,

    /// Invalid URL scheme.
    #[error("Invalid URL scheme for download: {scheme}")]
    InvalidScheme {
        /// The invalid scheme.
        scheme: String,
    },
}

impl DownloadError {
    /// Check if this error is a size limit exceeded error.
    pub fn is_size_limit(&self) -> bool {
        matches!(self, Self::SizeLimitExceeded { .. })
    }

    /// Check if this error is a cancellation error.
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }
}

/// Result of a download operation.
#[derive(Debug)]
pub struct DownloadResult {
    /// The downloaded data.
    pub data: Vec<u8>,
    /// The media type from the Content-Type header.
    pub media_type: Option<String>,
}

impl DownloadResult {
    /// Create a new download result.
    pub fn new(data: Vec<u8>, media_type: Option<String>) -> Self {
        Self { data, media_type }
    }
}

/// Download function type.
pub type DownloadFn = Box<
    dyn Fn(
            reqwest::Url,
            Option<CancellationToken>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<DownloadResult, DownloadError>> + Send>,
        > + Send
        + Sync,
>;

/// Options for creating a download function.
#[derive(Debug, Clone)]
pub struct DownloadOptions {
    /// Maximum allowed download size in bytes.
    pub max_bytes: u64,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_MAX_DOWNLOAD_SIZE,
        }
    }
}

impl DownloadOptions {
    /// Create new download options with a custom max size.
    pub fn with_max_bytes(max_bytes: u64) -> Self {
        Self { max_bytes }
    }
}

/// Validate that a URL scheme is allowed for downloading.
///
/// Only HTTP and HTTPS schemes are allowed.
pub fn validate_download_url(url: &str) -> Result<(), DownloadError> {
    let parsed = reqwest::Url::parse(url).map_err(|_| DownloadError::InvalidScheme {
        scheme: "invalid".to_string(),
    })?;

    match parsed.scheme() {
        "http" | "https" => Ok(()),
        scheme => Err(DownloadError::InvalidScheme {
            scheme: scheme.to_string(),
        }),
    }
}

/// Download content from a URL.
///
/// # Arguments
///
/// * `url` - The URL to download from
/// * `max_bytes` - Maximum allowed download size (defaults to 2 GiB)
/// * `abort_signal` - Optional cancellation token
///
/// # Returns
///
/// A `DownloadResult` containing the downloaded data and media type.
///
/// # Errors
///
/// Returns a `DownloadError` if the download fails, exceeds the size limit,
/// or is cancelled.
///
/// # Example
///
/// ```ignore
/// use vercel_ai::util::download::{download, DownloadOptions};
///
/// let result = download(
///     "https://example.com/video.mp4".parse().unwrap(),
///     Some(1024 * 1024), // 1 MB limit
///     None,
/// ).await?;
///
/// println!("Downloaded {} bytes", result.data.len());
/// ```
pub async fn download(
    url: reqwest::Url,
    max_bytes: Option<u64>,
    abort_signal: Option<CancellationToken>,
) -> Result<DownloadResult, DownloadError> {
    let url_str = url.to_string();
    let max_bytes = max_bytes.unwrap_or(DEFAULT_MAX_DOWNLOAD_SIZE);

    // Validate URL scheme
    validate_download_url(&url_str)?;

    // Check for cancellation before starting
    if let Some(ref token) = abort_signal
        && token.is_cancelled()
    {
        return Err(DownloadError::Cancelled);
    }

    // Create client
    let client = Client::new();

    // Build request with user agent
    let request = client
        .get(url.clone())
        .header("User-Agent", "vercel-ai-rust/1.0");

    // Send request
    let response = request.send().await.map_err(|e| DownloadError::Network {
        url: url_str.clone(),
        source: e,
    })?;

    // Validate final URL after redirects
    if response.url() != &url {
        validate_download_url(response.url().as_str())?;
    }

    // Check status
    if !response.status().is_success() {
        return Err(DownloadError::Http {
            url: url_str,
            status_code: response.status().as_u16(),
            status_text: response
                .status()
                .canonical_reason()
                .unwrap_or("")
                .to_string(),
        });
    }

    // Get content type
    let media_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| {
            // Strip charset and other parameters
            s.split(';').next().unwrap_or(s).trim().to_string()
        });

    // Check content length if available
    if let Some(content_length) = response.content_length()
        && content_length > max_bytes
    {
        return Err(DownloadError::SizeLimitExceeded {
            url: url_str,
            max_bytes,
        });
    }

    // Download with size limit
    let mut total_read = 0u64;
    let mut data = Vec::new();

    use futures::StreamExt;

    let mut stream = response.bytes_stream();
    while let Some(chunk_result) = stream.next().await {
        // Check for cancellation
        if let Some(ref token) = abort_signal
            && token.is_cancelled()
        {
            return Err(DownloadError::Cancelled);
        }

        let chunk = chunk_result.map_err(|e| DownloadError::Network {
            url: url_str.clone(),
            source: e,
        })?;

        total_read += chunk.len() as u64;
        if total_read > max_bytes {
            return Err(DownloadError::SizeLimitExceeded {
                url: url_str,
                max_bytes,
            });
        }

        data.extend_from_slice(&chunk);
    }

    Ok(DownloadResult::new(data, media_type))
}

/// Create a download function with configurable options.
///
/// This returns a closure that can be passed to functions like
/// `generate_video` for custom download behavior.
///
/// # Arguments
///
/// * `options` - Download options including max size
///
/// # Returns
///
/// A closure that performs downloads with the configured options.
///
/// # Example
///
/// ```ignore
/// use vercel_ai::util::download::{create_download, DownloadOptions};
///
/// let download_fn = create_download(DownloadOptions::with_max_bytes(100 * 1024 * 1024));
///
/// // Use the download function
/// let result = download_fn(url, None).await?;
/// ```
#[allow(clippy::type_complexity)]
pub fn create_download(
    options: DownloadOptions,
) -> impl Fn(
    reqwest::Url,
    Option<CancellationToken>,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DownloadResult, DownloadError>> + Send>,
> + Send
+ Sync {
    let max_bytes = options.max_bytes;

    move |url, abort_signal| {
        let max_bytes = max_bytes;
        Box::pin(async move { download(url, Some(max_bytes), abort_signal).await })
    }
}

#[cfg(test)]
#[path = "download.test.rs"]
mod tests;
