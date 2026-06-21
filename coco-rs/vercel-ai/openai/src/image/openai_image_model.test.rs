use super::*;

fn make_config() -> Arc<OpenAIConfig> {
    Arc::new(OpenAIConfig {
        provider: "openai.image".into(),
        base_url: "https://api.openai.com/v1".into(),
        headers: Arc::new(|| {
            let mut h = std::collections::HashMap::new();
            h.insert("Authorization".into(), "Bearer test".into());
            h
        }),
        client: None,
        full_url: None,
        chatgpt_subscription: false,
        reasoning_store: Default::default(),
    })
}

#[test]
fn creates_model() {
    let model = OpenAIImageModel::new("dall-e-3", make_config());
    assert_eq!(model.model_id(), "dall-e-3");
    assert_eq!(model.provider(), "openai.image");
    assert_eq!(model.max_images_per_call(), 1);
}

#[test]
fn max_images_per_call_known_model() {
    let model = OpenAIImageModel::new("gpt-image-1", make_config());
    assert_eq!(model.max_images_per_call(), 10);
}

#[test]
fn max_images_per_call_unknown_defaults_to_one() {
    let model = OpenAIImageModel::new("future-model", make_config());
    assert_eq!(model.max_images_per_call(), 1);
}

#[test]
fn derive_media_type_png() {
    assert_eq!(derive_media_type(Some("png")), Some("image/png".into()));
}

#[test]
fn derive_media_type_jpeg() {
    assert_eq!(derive_media_type(Some("jpeg")), Some("image/jpeg".into()));
    assert_eq!(derive_media_type(Some("jpg")), Some("image/jpeg".into()));
}

#[test]
fn derive_media_type_webp() {
    assert_eq!(derive_media_type(Some("webp")), Some("image/webp".into()));
}

#[test]
fn derive_media_type_none_for_unknown() {
    assert_eq!(derive_media_type(Some("bmp")), None);
    assert_eq!(derive_media_type(None), None);
}

#[test]
fn file_to_bytes_base64() {
    let file = ImageModelV4File::File {
        media_type: "image/png".into(),
        data: ImageFileData::Base64("aGVsbG8=".into()), // "hello"
        provider_options: None,
    };
    let (bytes, mime) = file_to_bytes(&file).unwrap();
    assert_eq!(bytes, b"hello");
    assert_eq!(mime, "image/png");
}

#[test]
fn file_to_bytes_binary() {
    let file = ImageModelV4File::File {
        media_type: "image/jpeg".into(),
        data: ImageFileData::Binary(vec![0xFF, 0xD8]),
        provider_options: None,
    };
    let (bytes, mime) = file_to_bytes(&file).unwrap();
    assert_eq!(bytes, vec![0xFF, 0xD8]);
    assert_eq!(mime, "image/jpeg");
}

#[test]
fn file_to_bytes_url_returns_error() {
    let file = ImageModelV4File::Url {
        url: "https://example.com/image.png".into(),
        provider_options: None,
    };
    let result = file_to_bytes(&file);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("URL-based image files are not supported"));
}

#[test]
fn mime_to_ext_known() {
    assert_eq!(mime_to_ext("image/png"), "png");
    assert_eq!(mime_to_ext("image/jpeg"), "jpg");
    assert_eq!(mime_to_ext("image/webp"), "webp");
    assert_eq!(mime_to_ext("image/gif"), "gif");
}

#[test]
fn mime_to_ext_unknown() {
    assert_eq!(mime_to_ext("application/octet-stream"), "bin");
}
