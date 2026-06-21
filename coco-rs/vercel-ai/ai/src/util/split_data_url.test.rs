//! Tests for split_data_url.rs

use super::*;

#[test]
fn test_split_data_url_with_media_type() {
    let result = split_data_url("data:image/png;base64,iVBORw0KGgo=");
    assert_eq!(result.media_type, Some("image/png".to_string()));
    assert_eq!(result.base64_content, Some("iVBORw0KGgo=".to_string()));
}

#[test]
fn test_split_data_url_without_media_type() {
    let result = split_data_url("data:;base64,iVBORw0KGgo=");
    assert_eq!(result.media_type, None);
    assert_eq!(result.base64_content, Some("iVBORw0KGgo=".to_string()));
}

#[test]
fn test_split_data_url_plain() {
    let result = split_data_url("data:text/plain,Hello%20World");
    assert_eq!(result.media_type, Some("text/plain".to_string()));
    assert_eq!(result.base64_content, Some("Hello%20World".to_string()));
}

#[test]
fn test_split_data_url_invalid() {
    let result = split_data_url("not a data url");
    assert_eq!(result.media_type, None);
    assert_eq!(result.base64_content, None);
}

#[test]
fn test_split_data_url_no_comma() {
    let result = split_data_url("data:image/png;base64");
    assert_eq!(result.media_type, None);
    assert_eq!(result.base64_content, None);
}

#[test]
fn test_is_data_url() {
    assert!(is_data_url("data:image/png;base64,abc"));
    assert!(!is_data_url("http://example.com"));
    assert!(!is_data_url("not a url"));
}

#[test]
fn test_is_http_url() {
    assert!(is_http_url("http://example.com"));
    assert!(is_http_url("https://example.com"));
    assert!(!is_http_url("data:image/png;base64,abc"));
    assert!(!is_http_url("ftp://example.com"));
}
