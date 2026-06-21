//! Tests for smooth_stream.rs

use super::*;
use futures::StreamExt;

#[tokio::test]
async fn test_smooth_stream_basic() {
    let input = vec!["Hello".to_string(), " World".to_string()];
    let config = SmoothStreamConfig {
        min_chunk_delay: Duration::from_millis(0),
        max_chunk_size: 100,
    };
    let mut stream = smooth_stream_iter(input, config);

    let mut result = String::new();
    while let Some(chunk) = stream.next().await {
        result.push_str(&chunk);
    }

    assert_eq!(result, "Hello World");
}
