use super::*;

#[test]
fn test_convert_base64_to_bytes() {
    let bytes = convert_base64_to_bytes("SGVsbG8gV29ybGQ=");
    assert_eq!(bytes, b"Hello World");
}

#[test]
fn test_convert_base64_to_bytes_url_safe() {
    // Standard base64 test
    let bytes = convert_base64_to_bytes("SGVsbG8gV29ybGQ=");
    assert_eq!(bytes, b"Hello World");
}

#[test]
fn test_convert_bytes_to_base64() {
    let base64 = convert_bytes_to_base64(b"Hello World");
    assert_eq!(base64, "SGVsbG8gV29ybGQ=");
}

#[test]
fn test_convert_to_base64() {
    let base64 = convert_to_base64(b"test data");
    assert!(base64.starts_with("dGVzdCBkYXRh"));
}

#[test]
fn test_roundtrip() {
    let original = b"Hello, World! This is a test.";
    let base64 = convert_bytes_to_base64(original);
    let decoded = convert_base64_to_bytes(&base64);
    assert_eq!(&decoded, original);
}
