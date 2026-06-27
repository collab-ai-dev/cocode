//! Tests for image model types.

use super::*;

#[test]
fn test_image_size_dimensions() {
    assert_eq!(ImageSize::S256x256.dimensions(), (256, 256));
    assert_eq!(ImageSize::S512x512.dimensions(), (512, 512));
    assert_eq!(ImageSize::S1024x1024.dimensions(), (1024, 1024));
    assert_eq!(ImageSize::S1792x1024.dimensions(), (1792, 1024));
    assert_eq!(ImageSize::S1024x1792.dimensions(), (1024, 1792));
    assert_eq!(
        ImageSize::Custom {
            width: 800,
            height: 600
        }
        .dimensions(),
        (800, 600)
    );
}

#[test]
fn test_generated_image_url() {
    let image = GeneratedImage::url("https://example.com/image.png");
    assert!(image.as_url().is_some());
    assert_eq!(image.as_url().unwrap(), "https://example.com/image.png");
    assert!(image.as_base64().is_none());
}

#[test]
fn test_generated_image_base64() {
    let image = GeneratedImage::base64("aGVsbG8=").with_media_type("image/png");
    assert!(image.as_base64().is_some());
    assert_eq!(image.as_base64().unwrap(), "aGVsbG8=");
    assert!(image.as_url().is_none());
    assert_eq!(image.media_type, Some("image/png".to_string()));
}

#[test]
fn test_image_model_v4_generate_result() {
    let result = ImageModelV4GenerateResult::from_urls(vec![
        "https://example.com/image1.png".to_string(),
        "https://example.com/image2.png".to_string(),
    ]);
    assert_eq!(result.images.len(), 2);
    assert!(result.warnings.is_empty());
    assert!(result.provider_metadata.is_none());
    assert!(result.usage.is_none());
}

#[test]
fn test_image_model_v4_generate_result_with_warnings() {
    let result =
        ImageModelV4GenerateResult::from_urls(vec!["https://example.com/image.png".to_string()])
            .with_warnings(vec![Warning::other("Content filter applied")]);
    assert_eq!(result.warnings.len(), 1);
}

#[test]
fn test_image_model_v4_generate_result_with_response() {
    let response = ImageModelV4Response::new()
        .with_model_id("dall-e-3")
        .with_timestamp("2024-01-01T00:00:00Z");
    let result =
        ImageModelV4GenerateResult::from_urls(vec!["https://example.com/image.png".to_string()])
            .with_response(response);
    assert_eq!(result.response.model_id, Some("dall-e-3".to_string()));
    assert_eq!(
        result.response.timestamp,
        Some("2024-01-01T00:00:00Z".to_string())
    );
}

#[test]
fn test_image_model_v4_generate_result_with_usage() {
    let usage = ImageModelV4Usage::new(25);
    let result =
        ImageModelV4GenerateResult::from_urls(vec!["https://example.com/image.png".to_string()])
            .with_usage(usage);
    assert!(result.usage.is_some());
    assert_eq!(result.usage.unwrap().prompt_tokens, 25);
}

#[test]
fn test_image_model_v4_call_options() {
    let options = ImageModelV4CallOptions::new("A cat sleeping on a couch")
        .with_n(2)
        .with_size(ImageSize::S1024x1024)
        .with_quality(ImageQuality::Hd)
        .with_style(ImageStyle::Natural);
    assert_eq!(options.prompt, "A cat sleeping on a couch");
    assert_eq!(options.n, Some(2));
    assert_eq!(options.size, Some(ImageSize::S1024x1024));
    assert_eq!(options.quality, Some(ImageQuality::Hd));
    assert_eq!(options.style, Some(ImageStyle::Natural));
}

#[test]
fn test_image_quality_default() {
    let quality = ImageQuality::default();
    assert!(matches!(quality, ImageQuality::Standard));
}

#[test]
fn test_image_style_default() {
    let style = ImageStyle::default();
    assert!(matches!(style, ImageStyle::Vivid));
}

#[test]
fn test_image_response_format_default() {
    let format = ImageResponseFormat::default();
    assert!(matches!(format, ImageResponseFormat::Url));
}
