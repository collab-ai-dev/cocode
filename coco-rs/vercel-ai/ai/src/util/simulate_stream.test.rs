use super::*;
use futures::StreamExt;

#[tokio::test]
async fn test_simulate_readable_stream() {
    let items = vec![1, 2, 3];
    let stream = simulate_readable_stream(items, None);
    let collected: Vec<_> = stream.collect().await;

    assert_eq!(collected, vec![1, 2, 3]);
}

#[tokio::test]
async fn test_simulate_interval_stream() {
    let stream = simulate_interval_stream(3, Duration::from_millis(1), |i| i * 2);
    let collected: Vec<_> = stream.collect().await;

    assert_eq!(collected, vec![0, 2, 4]);
}

#[test]
fn test_simulated_stream() {
    let mut stream = SimulatedStream::new(vec![1, 2, 3]);

    assert_eq!(stream.next(), Some(1));
    assert_eq!(stream.next(), Some(2));
    assert_eq!(stream.next(), Some(3));
    assert_eq!(stream.next(), None);
    assert!(stream.is_finished());
}

#[test]
fn test_simulated_stream_pause() {
    let mut stream = SimulatedStream::new(vec![1, 2, 3]);

    assert_eq!(stream.next(), Some(1));
    stream.pause();
    assert_eq!(stream.next(), None);
    stream.resume();
    assert_eq!(stream.next(), Some(2));
}