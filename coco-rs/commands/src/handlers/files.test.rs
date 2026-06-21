use super::*;

#[tokio::test]
async fn test_files_handler() {
    let result = handler("".to_string()).await.unwrap();
    // Either shows tracked files or a not-in-repo message
    assert!(!result.is_empty());
}

#[tokio::test]
async fn test_files_with_filter() {
    let result = handler("*.rs".to_string()).await.unwrap();
    assert!(!result.is_empty());
}
