use super::*;

#[tokio::test]
async fn test_memory_list() {
    let result = handler("".to_string()).await.unwrap();
    assert!(result.contains("Memory Files"));
}

#[tokio::test]
async fn test_memory_refresh() {
    let result = handler("refresh".to_string()).await.unwrap();
    assert!(result.contains("reload"));
}
