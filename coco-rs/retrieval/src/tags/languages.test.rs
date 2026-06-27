use super::*;

#[test]
fn test_from_extension() {
    assert_eq!(
        SupportedLanguage::from_extension("rs"),
        Some(SupportedLanguage::Rust)
    );
    assert_eq!(
        SupportedLanguage::from_extension("go"),
        Some(SupportedLanguage::Go)
    );
    assert_eq!(
        SupportedLanguage::from_extension("py"),
        Some(SupportedLanguage::Python)
    );
    assert_eq!(
        SupportedLanguage::from_extension("java"),
        Some(SupportedLanguage::Java)
    );
    assert_eq!(SupportedLanguage::from_extension("unknown"), None);
}

#[test]
fn test_from_path() {
    assert_eq!(
        SupportedLanguage::from_path(Path::new("main.rs")),
        Some(SupportedLanguage::Rust)
    );
    assert_eq!(
        SupportedLanguage::from_path(Path::new("main.go")),
        Some(SupportedLanguage::Go)
    );
    assert_eq!(
        SupportedLanguage::from_path(Path::new("script.py")),
        Some(SupportedLanguage::Python)
    );
}

#[test]
fn test_tags_configuration() {
    // Test that we can create tags configuration for each language
    for lang in [
        SupportedLanguage::Rust,
        SupportedLanguage::Go,
        SupportedLanguage::Python,
        SupportedLanguage::Java,
    ] {
        let result = lang.tags_configuration();
        assert!(
            result.is_ok(),
            "Failed to create config for {lang}: {result:?}"
        );
    }
}
