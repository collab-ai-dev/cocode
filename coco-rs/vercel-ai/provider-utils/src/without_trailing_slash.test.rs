//! Tests for without_trailing_slash module.

use super::*;

#[test]
fn test_without_trailing_slash_url() {
    assert_eq!(
        without_trailing_slash("https://example.com/"),
        "https://example.com"
    );
    assert_eq!(
        without_trailing_slash("https://example.com/api/"),
        "https://example.com/api"
    );
    assert_eq!(
        without_trailing_slash("https://example.com"),
        "https://example.com"
    );
}

#[test]
fn test_without_trailing_slash_path() {
    assert_eq!(
        without_trailing_slash("/path/to/resource/"),
        "/path/to/resource"
    );
    assert_eq!(
        without_trailing_slash("/path/to/resource"),
        "/path/to/resource"
    );
    assert_eq!(without_trailing_slash("/api/v1/"), "/api/v1");
}

#[test]
fn test_without_trailing_slash_root() {
    // Root path "/" should remain unchanged
    assert_eq!(without_trailing_slash("/"), "/");
}

#[test]
fn test_without_trailing_slash_multiple_slashes() {
    // Only removes one trailing slash
    assert_eq!(without_trailing_slash("path//"), "path/");
}

#[test]
fn test_without_trailing_slash_empty() {
    assert_eq!(without_trailing_slash(""), "");
}

#[test]
fn test_with_trailing_slash_url() {
    assert_eq!(
        with_trailing_slash("https://example.com"),
        "https://example.com/"
    );
    assert_eq!(
        with_trailing_slash("https://example.com/"),
        "https://example.com/"
    );
}

#[test]
fn test_with_trailing_slash_path() {
    assert_eq!(with_trailing_slash("/path"), "/path/");
    assert_eq!(with_trailing_slash("/path/"), "/path/");
}

#[test]
fn test_normalize_url() {
    assert_eq!(
        normalize_url("https://api.example.com/v1/"),
        "https://api.example.com/v1"
    );
    assert_eq!(
        normalize_url("https://api.example.com/v1"),
        "https://api.example.com/v1"
    );
}
