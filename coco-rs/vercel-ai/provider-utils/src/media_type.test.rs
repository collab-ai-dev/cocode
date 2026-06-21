use super::*;

#[test]
fn test_media_type_constants() {
    assert_eq!(MediaType::JSON.as_str(), "application/json");
    assert_eq!(MediaType::IMAGE_PNG.as_str(), "image/png");
}

#[test]
fn test_is_image() {
    assert!(MediaType::IMAGE_PNG.is_image());
    assert!(MediaType::IMAGE_JPEG.is_image());
    assert!(!MediaType::JSON.is_image());
}

#[test]
fn test_is_text() {
    assert!(MediaType::TEXT_PLAIN.is_text());
    assert!(MediaType::JSON.is_text());
    assert!(!MediaType::IMAGE_PNG.is_text());
}

#[test]
fn test_extension() {
    assert_eq!(MediaType::JSON.extension(), Some("json"));
    assert_eq!(MediaType::IMAGE_PNG.extension(), Some("png"));
}

#[test]
fn test_media_type_from_extension() {
    assert_eq!(media_type_from_extension("json"), MediaType::JSON);
    assert_eq!(media_type_from_extension("png"), MediaType::IMAGE_PNG);
    assert_eq!(
        media_type_from_extension("unknown"),
        MediaType::OCTET_STREAM
    );
}

#[test]
fn test_media_type_from_filename() {
    assert_eq!(media_type_from_filename("data.json"), MediaType::JSON);
    assert_eq!(media_type_from_filename("image.png"), MediaType::IMAGE_PNG);
    assert_eq!(
        media_type_from_filename("/path/to/file.pdf"),
        MediaType::PDF
    );
}

#[test]
fn test_display() {
    assert_eq!(format!("{}", MediaType::JSON), "application/json");
}
