use super::*;

#[tokio::test]
async fn test_stats_handler() {
    let result = handler("".to_string()).await.unwrap();
    assert!(result.contains("Session"));
}
