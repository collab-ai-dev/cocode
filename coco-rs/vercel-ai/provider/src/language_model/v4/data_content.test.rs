use super::*;

#[test]
fn test_data_content_bytes() {
    let content = LanguageModelV4DataContent::bytes(vec![1, 2, 3, 4]);
    assert!(content.is_bytes());
    assert!(!content.is_base64());
    assert!(!content.is_url());
    assert_eq!(content.as_bytes(), Some(&[1, 2, 3, 4][..]));
}

#[test]
fn test_data_content_base64() {
    let content = LanguageModelV4DataContent::base64("c2FtcGxl");
    assert!(!content.is_bytes());
    assert!(content.is_base64());
    assert!(!content.is_url());
    assert_eq!(content.as_base64(), Some("c2FtcGxl"));
}

#[test]
fn test_data_content_url() {
    let content = LanguageModelV4DataContent::url("https://example.com/file.png");
    assert!(!content.is_bytes());
    assert!(!content.is_base64());
    assert!(content.is_url());
    assert_eq!(content.as_url(), Some("https://example.com/file.png"));
}

#[test]
fn test_data_content_from_bytes() {
    let content: LanguageModelV4DataContent = vec![1, 2, 3].into();
    assert!(content.is_bytes());
}

#[test]
fn test_data_content_from_string_url() {
    let content: LanguageModelV4DataContent = "https://example.com/image.png".into();
    assert!(content.is_url());
}

#[test]
fn test_data_content_from_string_base64() {
    let content: LanguageModelV4DataContent = "c29tZWRhdGE=".into();
    assert!(content.is_base64());
}

#[test]
fn test_data_content_serialization() {
    let content = LanguageModelV4DataContent::base64("test123");
    let json = serde_json::to_string(&content).unwrap();
    assert_eq!(json, r#""test123""#);
}

#[test]
fn test_data_content_url_serialization() {
    let content = LanguageModelV4DataContent::url("https://example.com/file");
    let json = serde_json::to_string(&content).unwrap();
    assert_eq!(json, r#""https://example.com/file""#);
}

#[test]
fn test_data_content_deserialization() {
    let content: LanguageModelV4DataContent = serde_json::from_str(r#""base64data""#).unwrap();
    assert!(content.is_base64());

    let content: LanguageModelV4DataContent =
        serde_json::from_str(r#""https://example.com/file""#).unwrap();
    assert!(content.is_url());
}
