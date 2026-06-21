use super::*;

#[tokio::test]
async fn test_stream_from_vec() {
    let items = vec![1, 2, 3];
    let stream = stream_from_vec(items);
    let collected: Vec<_> = stream.collect().await;

    assert_eq!(collected, vec![1, 2, 3]);
}

#[tokio::test]
async fn test_stream_from_range() {
    let stream = stream_from_range(0, 3);
    let collected: Vec<_> = stream.collect().await;

    assert_eq!(collected, vec![0, 1, 2]);
}

#[tokio::test]
async fn test_stream_repeat() {
    let stream = stream_repeat(42, 3);
    let collected: Vec<_> = stream.collect().await;

    assert_eq!(collected, vec![42, 42, 42]);
}