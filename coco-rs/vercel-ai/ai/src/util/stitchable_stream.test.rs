use super::*;
use futures::StreamExt;

#[tokio::test]
async fn test_stitchable_stream() {
    let stream1 = futures::stream::iter(vec![1, 2]);
    let stream2 = futures::stream::iter(vec![3, 4]);

    let stitchable = StitchableStream::from_stream(stream1).stitch(stream2);
    let collected: Vec<_> = stitchable.into_stream().collect().await;

    assert_eq!(collected.len(), 4);
}

#[tokio::test]
async fn test_stitch_streams() {
    let stream1 = futures::stream::iter(vec!['a', 'b']);
    let stream2 = futures::stream::iter(vec!['c', 'd']);

    let stitched = stitch_streams(vec![stream1, stream2]);
    let collected: Vec<_> = stitched.collect().await;

    assert_eq!(collected.len(), 4);
}