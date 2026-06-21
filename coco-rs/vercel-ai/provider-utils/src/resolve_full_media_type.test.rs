use super::*;
use vercel_ai_provider::FilePart;
use vercel_ai_provider::SharedV4FileData;

#[test]
fn returns_full_media_type_unchanged() {
    let part = FilePart::from_bytes(vec![0x89, 0x50, 0x4e, 0x47], "image/png");
    let resolved = resolve_full_media_type(&part).unwrap();
    assert_eq!(resolved, "image/png");
}

#[test]
fn detects_png_from_bytes() {
    let part = FilePart::from_bytes(vec![0x89, 0x50, 0x4e, 0x47], "image");
    let resolved = resolve_full_media_type(&part).unwrap();
    assert_eq!(resolved, "image/png");
}

#[test]
fn detects_pdf_from_bytes() {
    let part = FilePart::from_bytes(vec![0x25, 0x50, 0x44, 0x46], "application");
    let resolved = resolve_full_media_type(&part).unwrap();
    assert_eq!(resolved, "application/pdf");
}

#[test]
fn detects_image_with_wildcard_subtype() {
    let part = FilePart::from_bytes(vec![0x89, 0x50, 0x4e, 0x47], "image/*");
    let resolved = resolve_full_media_type(&part).unwrap();
    assert_eq!(resolved, "image/png");
}

#[test]
fn errors_on_unsupported_top_level() {
    let part = FilePart::from_bytes(vec![0xab; 16], "text");
    let err = resolve_full_media_type(&part).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("could not be auto-detected"), "{msg}");
}

#[test]
fn errors_on_url_only_input() {
    let part = FilePart::new(SharedV4FileData::url("https://x"), "image");
    let err = resolve_full_media_type(&part).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("not passed as inline bytes"), "{msg}");
}

#[test]
fn is_full_media_type_helper() {
    assert!(is_full_media_type("image/png"));
    assert!(!is_full_media_type("image/*"));
    assert!(!is_full_media_type("image"));
    assert!(!is_full_media_type("image/"));
    assert!(!is_full_media_type(""));
    assert!(!is_full_media_type("/"));
}

#[test]
fn get_top_level_media_type_helper() {
    assert_eq!(get_top_level_media_type("image/png"), "image");
    assert_eq!(get_top_level_media_type("image/*"), "image");
    assert_eq!(get_top_level_media_type("image"), "image");
    assert_eq!(get_top_level_media_type("image/"), "image");
    assert_eq!(get_top_level_media_type(""), "");
    assert_eq!(get_top_level_media_type("/"), "");
}
