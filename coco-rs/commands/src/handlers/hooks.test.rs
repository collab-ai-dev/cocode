use super::*;

#[tokio::test]
async fn test_hooks_no_args() {
    let result = handler("".to_string()).await.unwrap();
    // Either shows configured hooks or help text
    assert!(!result.is_empty());
}
