//! Tests for detect_media_type.rs

use super::*;

#[test]
fn test_detect_png() {
    let png_data = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
    assert_eq!(
        detect_media_type(&png_data, IMAGE_MEDIA_TYPE_SIGNATURES),
        Some("image/png")
    );
}

#[test]
fn test_detect_jpeg() {
    let jpeg_data = [0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10];
    assert_eq!(
        detect_media_type(&jpeg_data, IMAGE_MEDIA_TYPE_SIGNATURES),
        Some("image/jpeg")
    );
}

#[test]
fn test_detect_gif() {
    let gif_data = [0x47, 0x49, 0x46, 0x38, 0x39, 0x61];
    assert_eq!(
        detect_media_type(&gif_data, IMAGE_MEDIA_TYPE_SIGNATURES),
        Some("image/gif")
    );
}

#[test]
fn test_detect_mp4() {
    let mp4_data = [0x00, 0x00, 0x00, 0x20, 0x66, 0x74, 0x79, 0x70];
    assert_eq!(
        detect_media_type(&mp4_data, VIDEO_MEDIA_TYPE_SIGNATURES),
        Some("video/mp4")
    );
}

#[test]
fn test_detect_webm() {
    let webm_data = [0x1a, 0x45, 0xdf, 0xa3, 0x00, 0x00, 0x00];
    assert_eq!(
        detect_media_type(&webm_data, VIDEO_MEDIA_TYPE_SIGNATURES),
        Some("video/webm")
    );
}

#[test]
fn test_detect_audio_mpeg() {
    let mp3_data = [0xff, 0xfb, 0x90, 0x00];
    assert_eq!(
        detect_media_type(&mp3_data, AUDIO_MEDIA_TYPE_SIGNATURES),
        Some("audio/mpeg")
    );
}

#[test]
fn test_unknown_format() {
    let unknown_data = [0x00, 0x00, 0x00, 0x00];
    assert_eq!(
        detect_media_type(&unknown_data, IMAGE_MEDIA_TYPE_SIGNATURES),
        None
    );
}
