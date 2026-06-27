use super::*;

#[test]
fn test_data_content_from_string_base64() {
    let content = DataContentValue::from_string("SGVsbG8=");
    assert!(matches!(content, DataContentValue::Base64(_)));
}

#[test]
fn test_data_content_from_string_https_url() {
    let content = DataContentValue::from_string("https://example.com/image.png");
    assert!(matches!(content, DataContentValue::Url(_)));
}

#[test]
fn test_data_content_from_string_data_url() {
    let content = DataContentValue::from_string("data:image/png;base64,SGVsbG8=");
    assert!(matches!(content, DataContentValue::Url(_)));
}

#[test]
fn test_data_content_from_binary() {
    let content = DataContentValue::from_binary(vec![1, 2, 3]);
    assert!(matches!(content, DataContentValue::Binary(_)));
}

#[test]
fn test_base64_to_base64_roundtrip() {
    let content = DataContentValue::Base64("SGVsbG8=".to_string());
    let result = content.to_base64().unwrap();
    assert_eq!(result, "SGVsbG8=");
}

#[test]
fn test_binary_to_base64() {
    let content = DataContentValue::from_binary(b"Hello".to_vec());
    let result = content.to_base64().unwrap();
    assert_eq!(result, "SGVsbG8=");
}

#[test]
fn test_binary_to_binary_roundtrip() {
    let data = vec![1, 2, 3, 4, 5];
    let content = DataContentValue::from_binary(data.clone());
    let result = content.to_binary().unwrap();
    assert_eq!(result, data);
}

#[test]
fn test_base64_to_binary() {
    let content = DataContentValue::Base64("SGVsbG8=".to_string());
    let result = content.to_binary().unwrap();
    assert_eq!(result, b"Hello");
}

#[test]
fn test_data_url_to_base64() {
    let content = DataContentValue::Url("data:image/png;base64,SGVsbG8=".to_string());
    let result = content.to_base64().unwrap();
    assert_eq!(result, "SGVsbG8=");
}

#[test]
fn test_http_url_to_base64_error() {
    let content = DataContentValue::Url("https://example.com/image.png".to_string());
    let result = content.to_base64();
    assert!(result.is_err());
}

#[test]
fn test_convert_to_lm_data_content_binary() {
    let data = vec![1, 2, 3];
    let (result_data, media_type) =
        convert_to_language_model_data_content(DataContentValue::Binary(data.clone())).unwrap();
    assert_eq!(result_data, data);
    assert!(media_type.is_none());
}

#[test]
fn test_convert_to_lm_data_content_base64() {
    let (data, media_type) =
        convert_to_language_model_data_content(DataContentValue::Base64("SGVsbG8=".to_string()))
            .unwrap();
    assert_eq!(data, b"Hello");
    assert!(media_type.is_none());
}

#[test]
fn test_convert_to_lm_data_content_data_url() {
    let (data, media_type) = convert_to_language_model_data_content(DataContentValue::Url(
        "data:image/png;base64,SGVsbG8=".to_string(),
    ))
    .unwrap();
    assert_eq!(data, b"Hello");
    assert_eq!(media_type, Some("image/png".to_string()));
}

#[test]
fn test_convert_uint8_array_to_text_valid() {
    let result = convert_uint8_array_to_text(b"Hello, world!").unwrap();
    assert_eq!(result, "Hello, world!");
}

#[test]
fn test_convert_uint8_array_to_text_invalid_utf8() {
    let result = convert_uint8_array_to_text(&[0xFF, 0xFE]);
    assert!(result.is_err());
}
