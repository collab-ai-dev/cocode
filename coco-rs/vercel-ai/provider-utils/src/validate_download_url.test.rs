//! Tests for validate_download_url module.

use super::*;

#[test]
fn test_validate_download_url_valid_https() {
    let result = validate_download_url("https://example.com/file.zip");
    assert!(result.is_ok());
    let url = result.unwrap();
    assert_eq!(url.scheme(), "https");
    assert_eq!(url.host_str(), Some("example.com"));
}

#[test]
fn test_validate_download_url_valid_http() {
    let result = validate_download_url("http://example.com/file.zip");
    assert!(result.is_ok());
    let url = result.unwrap();
    assert_eq!(url.scheme(), "http");
}

#[test]
fn test_validate_download_url_invalid_protocol() {
    let result = validate_download_url("ftp://example.com/file.zip");
    assert!(matches!(result, Err(DownloadUrlError::InvalidProtocol)));

    let result = validate_download_url("file:///path/to/file");
    assert!(matches!(result, Err(DownloadUrlError::InvalidProtocol)));
}

#[test]
fn test_validate_download_url_data_urls_are_valid() {
    let result = validate_download_url("data:text/plain,hello");
    assert!(result.is_ok());
    assert_eq!(result.unwrap().scheme(), "data");

    let result = validate_download_url("data:image/png;base64,abc123");
    assert!(result.is_ok());
}

#[test]
fn test_validate_download_url_invalid_url() {
    let result = validate_download_url("not-a-url");
    assert!(matches!(result, Err(DownloadUrlError::InvalidUrl)));

    let result = validate_download_url("://missing-scheme.com");
    assert!(matches!(result, Err(DownloadUrlError::InvalidUrl)));
}

#[test]
fn test_validate_download_url_with_port() {
    let result = validate_download_url("https://example.com:8080/file.zip");
    assert!(result.is_ok());
}

#[test]
fn test_validate_download_url_with_query() {
    let result = validate_download_url("https://example.com/file.zip?token=abc123");
    assert!(result.is_ok());
}

#[test]
fn test_validate_download_url_with_fragment() {
    let result = validate_download_url("https://example.com/file.zip#section");
    assert!(result.is_ok());
}

#[test]
fn test_is_valid_download_url() {
    assert!(is_valid_download_url("https://example.com/file.zip"));
    assert!(is_valid_download_url("http://example.com/file.zip"));
    assert!(!is_valid_download_url("ftp://example.com/file.zip"));
    assert!(!is_valid_download_url("not-a-url"));
}
