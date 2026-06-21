use super::*;

#[test]
fn test_source_url() {
    let source = LanguageModelV4Source::url("src-1", "https://example.com");
    match source {
        LanguageModelV4Source::Url { id, url, .. } => {
            assert_eq!(id, "src-1");
            assert_eq!(url, "https://example.com");
        }
        _ => panic!("Expected URL source"),
    }
}

#[test]
fn test_source_document() {
    let source = LanguageModelV4Source::document("doc-1", "application/pdf", "My Document");
    match source {
        LanguageModelV4Source::Document {
            id,
            media_type,
            title,
            ..
        } => {
            assert_eq!(id, "doc-1");
            assert_eq!(media_type, "application/pdf");
            assert_eq!(title, "My Document");
        }
        _ => panic!("Expected Document source"),
    }
}

#[test]
fn test_source_url_serialization() {
    let source = LanguageModelV4Source::url("src-1", "https://example.com");
    let json = serde_json::to_string(&source).unwrap();
    assert!(json.contains(r#""sourceType":"url"#));
    assert!(json.contains(r#""url":"https://example.com"#));
}

#[test]
fn test_source_document_serialization() {
    let source = LanguageModelV4Source::document("doc-1", "application/pdf", "Test Doc");
    let json = serde_json::to_string(&source).unwrap();
    assert!(json.contains(r#""sourceType":"document"#));
    assert!(json.contains(r#""mediaType":"application/pdf"#));
}

#[test]
fn test_source_with_title() {
    let source =
        LanguageModelV4Source::url("src-1", "https://example.com").with_title("Example Site");
    match source {
        LanguageModelV4Source::Url { title, .. } => {
            assert_eq!(title, Some("Example Site".to_string()));
        }
        _ => panic!("Expected URL source"),
    }
}

#[test]
fn test_source_type_serialization() {
    assert_eq!(serde_json::to_string(&SourceType::Url).unwrap(), r#""url""#);
    assert_eq!(
        serde_json::to_string(&SourceType::Document).unwrap(),
        r#""document""#
    );
}
