use super::*;
use pretty_assertions::assert_eq;

#[test]
fn recognizes_supported_models() {
    assert!(is_browser_search_supported_model("openai/gpt-oss-20b"));
    assert!(is_browser_search_supported_model("openai/gpt-oss-120b"));
}

#[test]
fn rejects_unsupported_models() {
    assert!(!is_browser_search_supported_model(
        "llama-3.3-70b-versatile"
    ));
    assert!(!is_browser_search_supported_model("qwen/qwen3-32b"));
}

#[test]
fn lists_supported_models() {
    assert_eq!(
        supported_models_string(),
        "openai/gpt-oss-20b, openai/gpt-oss-120b"
    );
}
