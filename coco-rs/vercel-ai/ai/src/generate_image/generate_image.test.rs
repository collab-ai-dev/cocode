use super::*;

#[test]
fn test_generate_image_options() {
    let options = GenerateImageOptions::new("dall-e-3", "A sunset")
        .with_n(2)
        .with_size(ImageSize::S1024x1024);

    assert!(options.model.is_string());
    assert_eq!(options.n, Some(2));
    assert_eq!(options.size, Some(ImageSize::S1024x1024));
}

#[test]
fn test_image_size() {
    assert_eq!(ImageSize::S1024x1024.dimensions(), (1024, 1024));
    assert_eq!(ImageSize::S1792x1024.dimensions(), (1792, 1024));

    let custom = ImageSize::Custom {
        width: 800,
        height: 600,
    };
    assert_eq!(custom.dimensions(), (800, 600));

    // Test parsing
    let parsed = ImageSize::parse("800x600");
    assert!(parsed.is_some());
    assert_eq!(parsed.unwrap().dimensions(), (800, 600));
}

#[test]
fn test_image_prompt() {
    let prompt: ImagePrompt = "A beautiful landscape".into();
    match prompt {
        ImagePrompt::Text(text) => assert_eq!(text, "A beautiful landscape"),
        _ => panic!("Expected Text variant"),
    }
}

#[test]
fn test_generated_image() {
    let url_img = GeneratedImage::url("https://example.com/image.png");
    assert!(url_img.as_url().is_some());
    assert!(url_img.as_base64().is_none());

    let b64_img = GeneratedImage::base64("aGVsbG8=").with_media_type("image/png");
    assert!(b64_img.as_base64().is_some());
    assert_eq!(b64_img.media_type, Some("image/png".to_string()));
    assert_eq!(b64_img.extension(), "png");
}
