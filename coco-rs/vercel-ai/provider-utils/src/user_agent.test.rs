use super::*;

#[test]
fn test_build_user_agent() {
    let ua = build_user_agent("openai", "1.0.0");
    assert!(ua.contains("vercel-ai-sdk-rust"));
    assert!(ua.contains("openai/1.0.0"));
}

#[test]
fn test_build_simple_user_agent() {
    let ua = build_simple_user_agent("anthropic", "2.0.0");
    assert!(ua.starts_with("vercel-ai-sdk-rust/"));
    assert!(ua.contains("anthropic/2.0.0"));
}

#[test]
fn test_build_custom_user_agent() {
    let ua = build_custom_user_agent("my-sdk", "3.0.0", "my-provider", "1.0.0");
    assert!(ua.contains("my-sdk/3.0.0"));
    assert!(ua.contains("my-provider/1.0.0"));
}
