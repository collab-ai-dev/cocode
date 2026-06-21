use super::*;

#[test]
fn test_parse_data_url() {
    let url = "data:text/plain;base64,SGVsbG8gV29ybGQ=";
    let parsed = parse_data_url(url).unwrap();
    assert_eq!(parsed.media_type, Some("text/plain".to_string()));
    assert!(parsed.is_base64);
    assert_eq!(parsed.data, "SGVsbG8gV29ybGQ=");

    let decoded = parsed.decode().unwrap();
    assert_eq!(String::from_utf8_lossy(&decoded), "Hello World");
}

#[test]
fn test_parse_data_url_image() {
    let url = "data:image/png;base64,iVBORw0KGgo=";
    let parsed = parse_data_url(url).unwrap();
    assert_eq!(parsed.media_type, Some("image/png".to_string()));
    assert!(parsed.is_base64);
}

#[test]
fn test_parse_data_url_no_encoding() {
    let url = "data:text/plain,Hello%20World";
    let parsed = parse_data_url(url).unwrap();
    assert_eq!(parsed.media_type, Some("text/plain".to_string()));
    assert!(!parsed.is_base64);

    let decoded = parsed.decode().unwrap();
    assert_eq!(String::from_utf8_lossy(&decoded), "Hello World");
}

#[test]
fn test_parse_data_url_invalid() {
    assert!(parse_data_url("http://example.com").is_none());
    assert!(parse_data_url("data:").is_none());
}

#[test]
fn test_is_url_supported() {
    let patterns = image_url_patterns();

    assert!(is_url_supported("https://example.com/image.png", &patterns));
    assert!(is_url_supported("https://example.com/image.jpg", &patterns));
    assert!(is_url_supported("data:image/png;base64,abc", &patterns));
    assert!(!is_url_supported("https://example.com/file.txt", &patterns));
}
