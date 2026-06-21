use super::*;

#[tokio::test]
async fn test_clear_default() {
    let result = handler("".to_string()).await.unwrap();
    // `/clear` is the full reset, status text reflects
    // that plan state + caches are also cleared.
    assert!(result.contains("Conversation cleared"));
    assert!(result.contains("Plan state"));
}

#[tokio::test]
async fn test_clear_ignores_args() {
    let result = handler("foobar".to_string()).await.unwrap();
    assert!(result.contains("Conversation cleared"));
    assert!(result.contains("Plan state"));
}
