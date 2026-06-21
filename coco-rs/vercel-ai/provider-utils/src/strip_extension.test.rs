//! Tests for strip_extension module.

use super::*;

#[test]
fn test_strip_extension_simple() {
    assert_eq!(strip_extension("file.txt"), "file");
    assert_eq!(strip_extension("file.json"), "file");
    assert_eq!(strip_extension("file.md"), "file");
}

#[test]
fn test_strip_extension_double() {
    // Only removes the last extension
    assert_eq!(strip_extension("file.tar.gz"), "file.tar");
    assert_eq!(strip_extension("archive.tar.bz2"), "archive.tar");
}

#[test]
fn test_strip_extension_no_extension() {
    assert_eq!(strip_extension("file"), "file");
    assert_eq!(strip_extension("noext"), "noext");
}

#[test]
fn test_strip_extension_with_path() {
    assert_eq!(strip_extension("/path/to/file.txt"), "/path/to/file");
    assert_eq!(strip_extension("./relative/path.txt"), "./relative/path");
}

#[test]
fn test_strip_extension_hidden_file() {
    assert_eq!(strip_extension(".gitignore"), ".gitignore");
    assert_eq!(strip_extension(".env"), ".env");
}

#[test]
fn test_strip_specific_extension_matching() {
    assert_eq!(strip_specific_extension("file.txt", "txt"), Some("file"));
    assert_eq!(strip_specific_extension("file.json", "json"), Some("file"));
}

#[test]
fn test_strip_specific_extension_not_matching() {
    assert_eq!(strip_specific_extension("file.txt", "md"), None);
    assert_eq!(strip_specific_extension("file.txt", "json"), None);
}

#[test]
fn test_strip_specific_extension_case_insensitive() {
    // The implementation should be case insensitive
    assert_eq!(strip_specific_extension("file.TXT", "txt"), Some("file"));
    assert_eq!(strip_specific_extension("file.Txt", "TXT"), Some("file"));
}

#[test]
fn test_strip_specific_extension_no_extension() {
    assert_eq!(strip_specific_extension("file", "txt"), None);
}
