use super::*;

#[test]
fn test_parse_simple_text() {
    let uri = parse_data_uri("data:text/plain,Hello").unwrap();
    assert_eq!(uri.media_type, "text/plain");
    assert_eq!(uri.data, b"Hello");
    assert!(!uri.base64_encoded);
}

#[test]
fn test_parse_url_encoded() {
    let uri = parse_data_uri("data:text/plain,Hello%20World").unwrap();
    assert_eq!(uri.data, b"Hello World");
}

#[test]
fn test_parse_base64() {
    let uri = parse_data_uri("data:image/png;base64,iVBORw0KGgo=").unwrap();
    assert_eq!(uri.media_type, "image/png");
    assert!(uri.base64_encoded);
    assert!(!uri.data.is_empty());
}

#[test]
fn test_parse_no_media_type() {
    let uri = parse_data_uri("data:,Hello").unwrap();
    assert_eq!(uri.media_type, "text/plain");
}

#[test]
fn test_parse_invalid() {
    assert!(parse_data_uri("not-a-data-uri").is_none());
    assert!(parse_data_uri("data:text/plain").is_none()); // No comma
}

#[test]
fn test_encode_data_uri() {
    let uri = encode_data_uri("image/png", b"\x89PNG");
    assert!(uri.starts_with("data:image/png;base64,"));
}

#[test]
fn test_encode_text_uri() {
    let uri = encode_text_uri("text/plain", "Hello World");
    assert_eq!(uri, "data:text/plain,Hello%20World");
}

#[test]
fn test_data_uri_to_string() {
    let uri = DataUri::new_base64("image/png", b"\x89PNG".to_vec());
    let s = uri.to_uri();
    assert!(s.starts_with("data:image/png;base64,"));
}
