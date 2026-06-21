use super::*;
use futures::stream;

#[tokio::test]
async fn test_consume_empty_stream() {
    let s = stream::empty::<i32>();
    consume_stream(s).await;
}

#[tokio::test]
async fn test_consume_non_empty_stream() {
    let s = stream::iter(vec![1, 2, 3]);
    consume_stream(s).await;
}

#[tokio::test]
async fn test_consume_stream_side_effects() {
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();

    let s = stream::iter(vec![1, 2, 3]).map(move |x| {
        counter_clone.fetch_add(x, Ordering::SeqCst);
        x
    });

    consume_stream(s).await;
    assert_eq!(counter.load(Ordering::SeqCst), 6);
}
