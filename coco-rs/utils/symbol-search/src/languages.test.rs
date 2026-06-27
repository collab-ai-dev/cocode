use super::*;

#[test]
fn test_from_extension() {
    assert_eq!(
        SymbolLanguage::from_extension("rs"),
        Some(SymbolLanguage::Rust)
    );
    assert_eq!(
        SymbolLanguage::from_extension("go"),
        Some(SymbolLanguage::Go)
    );
    assert_eq!(
        SymbolLanguage::from_extension("py"),
        Some(SymbolLanguage::Python)
    );
    assert_eq!(
        SymbolLanguage::from_extension("java"),
        Some(SymbolLanguage::Java)
    );
    assert_eq!(
        SymbolLanguage::from_extension("ts"),
        Some(SymbolLanguage::TypeScript)
    );
    assert_eq!(
        SymbolLanguage::from_extension("tsx"),
        Some(SymbolLanguage::TypeScript)
    );
    assert_eq!(
        SymbolLanguage::from_extension("js"),
        Some(SymbolLanguage::TypeScript)
    );
    assert_eq!(SymbolLanguage::from_extension("unknown"), None);
}

#[test]
fn test_tags_configuration() {
    for lang in [
        SymbolLanguage::Rust,
        SymbolLanguage::Go,
        SymbolLanguage::Python,
        SymbolLanguage::Java,
        SymbolLanguage::TypeScript,
    ] {
        let result = lang.tags_configuration();
        assert!(
            result.is_ok(),
            "Failed to create config for {lang:?}: {result:?}"
        );
    }
}
