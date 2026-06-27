//! File download utilities.
//!
//! This module provides utilities for downloading files from URLs.

use bytes::Bytes;
use futures::Stream;
use std::path::Path;
use std::pin::Pin;
use vercel_ai_provider::AISdkError;

/// Download a file from a URL to a local path.
///
/// # Arguments
///
/// * `url` - The URL to download from.
/// * `path` - The local path to save the file to.
///
/// # Errors
///
/// Returns an error if the download fails or the file cannot be written.
pub async fn download_file(url: &str, path: impl AsRef<Path>) -> Result<(), AISdkError> {
    let response = reqwest::get(url).await.map_err(|e| {
        AISdkError::new(format!(
            "Download error: Failed to download from {url}: {e}"
        ))
    })?;

    if !response.status().is_success() {
        return Err(AISdkError::new(format!(
            "Download error: HTTP {} when downloading from {}",
            response.status(),
            url
        )));
    }

    let bytes = response.bytes().await.map_err(|e| {
        AISdkError::new(format!("Download error: Failed to read response body: {e}"))
    })?;

    // Create parent directories if needed
    if let Some(parent) = path.as_ref().parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            AISdkError::new(format!("Download error: Failed to create directory: {e}"))
        })?;
    }

    // Write the file
    tokio::fs::write(path.as_ref(), &bytes)
        .await
        .map_err(|e| AISdkError::new(format!("Download error: Failed to write file: {e}")))?;

    Ok(())
}

/// Download a file as bytes.
///
/// # Arguments
///
/// * `url` - The URL to download from.
///
/// # Errors
///
/// Returns an error if the download fails.
pub async fn download_bytes(url: &str) -> Result<Bytes, AISdkError> {
    let response = reqwest::get(url).await.map_err(|e| {
        AISdkError::new(format!(
            "Download error: Failed to download from {url}: {e}"
        ))
    })?;

    if !response.status().is_success() {
        return Err(AISdkError::new(format!(
            "Download error: HTTP {} when downloading from {}",
            response.status(),
            url
        )));
    }

    response
        .bytes()
        .await
        .map_err(|e| AISdkError::new(format!("Download error: Failed to read response body: {e}")))
}

/// Download a file as a stream.
///
/// # Arguments
///
/// * `url` - The URL to download from.
///
/// # Errors
///
/// Returns an error if the download fails to start.
pub async fn download_stream(
    url: &str,
) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>, AISdkError> {
    let response = reqwest::get(url).await.map_err(|e| {
        AISdkError::new(format!(
            "Download error: Failed to download from {url}: {e}"
        ))
    })?;

    if !response.status().is_success() {
        return Err(AISdkError::new(format!(
            "Download error: HTTP {} when downloading from {}",
            response.status(),
            url
        )));
    }

    Ok(Box::pin(response.bytes_stream()))
}

/// Download a file as a string.
///
/// # Arguments
///
/// * `url` - The URL to download from.
///
/// # Errors
///
/// Returns an error if the download fails or the content is not valid UTF-8.
pub async fn download_string(url: &str) -> Result<String, AISdkError> {
    let bytes = download_bytes(url).await?;

    String::from_utf8(bytes.to_vec())
        .map_err(|e| AISdkError::new(format!("Download error: Content is not valid UTF-8: {e}")))
}

#[cfg(test)]
#[path = "download.test.rs"]
mod tests;
