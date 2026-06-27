use super::*;

#[tokio::test]
async fn test_serial_executor() {
    let executor = SerialJobExecutor::<i32>::new();

    executor.submit(|| 1).await;
    executor.submit(|| 2).await;
    executor.submit(|| 3).await;

    let results = executor.run_all().await;
    assert_eq!(results, vec![1, 2, 3]);
}

#[tokio::test]
async fn test_pending_count() {
    let executor = SerialJobExecutor::<i32>::new();

    executor.submit(|| 1).await;
    executor.submit(|| 2).await;

    assert_eq!(executor.pending_count().await, 2);

    executor.run_all().await;

    assert_eq!(executor.pending_count().await, 0);
}