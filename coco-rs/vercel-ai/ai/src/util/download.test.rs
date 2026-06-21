//! Tests for download.rs

use super::*;

#[test]
fn test_validate_url_http() {
    assert!(validate_download_url("http://example.com").is_ok());
}

#[test]
fn test_validate_url_https() {
    assert!(validate_download_url("https://example.com").is_ok());
}

#[test]
fn test_validate_url_invalid_scheme() {
    assert!(matches!(
        validate_download_url("ftp://example.com"),
        Err(DownloadError::InvalidScheme { .. })
    ));
}

#[test]
fn test_validate_url_invalid_format() {
    assert!(matches!(
        validate_download_url("not a url"),
        Err(DownloadError::InvalidScheme { .. })
    ));
}

#[test]
fn test_download_options_default() {
    let options = DownloadOptions::default();
    assert_eq!(options.max_bytes, DEFAULT_MAX_DOWNLOAD_SIZE);
}

#[test]
fn test_download_options_custom() {
    let options = DownloadOptions::with_max_bytes(1024);
    assert_eq!(options.max_bytes, 1024);
}

#[test]
fn test_download_result() {
    let result = DownloadResult::new(vec![1, 2, 3], Some("video/mp4".to_string()));
    assert_eq!(result.data, vec![1, 2, 3]);
    assert_eq!(result.media_type, Some("video/mp4".to_string()));
}
