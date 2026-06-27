use super::*;

#[tokio::test]
async fn test_context_handler_is_runtime_placeholder() {
    let output = handler(String::new()).await.unwrap();
    assert!(output.contains("active session runtime"));
    assert!(!output.contains("200,000"));
    assert!(!output.contains("System prompt"));
}
