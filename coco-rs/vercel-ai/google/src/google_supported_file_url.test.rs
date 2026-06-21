use super::*;

#[test]
fn recognizes_google_files_api_url() {
    assert!(is_supported_file_url(
        "https://generativelanguage.googleapis.com/v1beta/files/abc123"
    ));
}

#[test]
fn recognizes_youtube_watch_url() {
    assert!(is_supported_file_url(
        "https://www.youtube.com/watch?v=abc123"
    ));
}

#[test]
fn recognizes_youtu_be_url() {
    assert!(is_supported_file_url("https://youtu.be/abc123"));
}

#[test]
fn rejects_random_url() {
    assert!(!is_supported_file_url("https://example.com/file.txt"));
}

#[test]
fn rejects_empty_string() {
    assert!(!is_supported_file_url(""));
}
