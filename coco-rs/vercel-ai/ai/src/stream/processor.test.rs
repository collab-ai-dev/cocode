use futures::stream;
use std::time::Duration;
use vercel_ai_provider::errors::AISdkError;
use vercel_ai_provider::language_model::v4::stream::LanguageModelV4StreamPart;

use super::*;

/// Helper: create a StreamProcessor from a vec of parts.
fn processor_from_parts(parts: Vec<LanguageModelV4StreamPart>) -> StreamProcessor {
    let stream: Vec<Result<LanguageModelV4StreamPart, AISdkError>> =
        parts.into_iter().map(Ok).collect();
    StreamProcessor::from_stream(Box::pin(stream::iter(stream)))
}

async fn metrics_after_first_part(part: LanguageModelV4StreamPart) -> StreamMetrics {
    let mut processor = processor_from_parts(vec![part]);
    processor.next().await.unwrap().unwrap();
    processor.metrics()
}

#[tokio::test]
async fn test_metrics_ttft_records_text_delta() {
    let metrics = metrics_after_first_part(LanguageModelV4StreamPart::TextDelta {
        id: "t1".into(),
        delta: "hello".into(),
        provider_metadata: None,
    })
    .await;

    assert!(metrics.ttft_ms.is_some());
}

#[tokio::test]
async fn test_metrics_ttft_records_reasoning_delta() {
    let metrics = metrics_after_first_part(LanguageModelV4StreamPart::ReasoningDelta {
        id: "r1".into(),
        delta: "thinking".into(),
        provider_metadata: None,
    })
    .await;

    assert!(metrics.ttft_ms.is_some());
}

#[tokio::test]
async fn test_metrics_ttft_records_tool_input_start() {
    let metrics = metrics_after_first_part(LanguageModelV4StreamPart::ToolInputStart {
        id: "tc1".into(),
        tool_name: "read".into(),
        provider_executed: None,
        dynamic: None,
        title: None,
        provider_metadata: None,
    })
    .await;

    assert!(metrics.ttft_ms.is_some());
}

#[tokio::test(start_paused = true)]
async fn test_metrics_records_stall_gap() {
    let stream = stream::unfold(0usize, |idx| async move {
        if idx == 1 {
            tokio::time::sleep(Duration::from_secs(31)).await;
        }
        let delta = match idx {
            0 => "first",
            1 => "second",
            _ => return None,
        };
        Some((
            Ok(LanguageModelV4StreamPart::TextDelta {
                id: "t1".into(),
                delta: delta.into(),
                provider_metadata: None,
            }),
            idx + 1,
        ))
    });
    let config = StreamProcessorConfig::default()
        .without_idle_timeout()
        .with_stall_threshold(Duration::from_secs(30));
    let mut processor = StreamProcessor::from_stream_with_config(Box::pin(stream), config);

    processor.next().await.unwrap().unwrap();
    processor.next().await.unwrap().unwrap();
    let metrics = processor.metrics();

    assert_eq!(metrics.stall_count, 1);
    assert_eq!(metrics.total_stall_ms, 31_000);
}

#[tokio::test(start_paused = true)]
async fn test_configurable_idle_timeout() {
    let stream = stream::unfold(0usize, |idx| async move {
        if idx == 0 {
            tokio::time::sleep(Duration::from_secs(6)).await;
        }
        if idx > 0 {
            return None;
        }
        Some((
            Ok(LanguageModelV4StreamPart::TextDelta {
                id: "t1".into(),
                delta: "late".into(),
                provider_metadata: None,
            }),
            idx + 1,
        ))
    });
    let config = StreamProcessorConfig::default().with_idle_timeout(Duration::from_secs(5));
    let mut processor = StreamProcessor::from_stream_with_config(Box::pin(stream), config);

    let err = processor.next().await.unwrap().unwrap_err();
    assert!(err.message.contains("idle timeout after 5s"));
}

#[tokio::test]
async fn test_stream_error_propagation() {
    let items: Vec<Result<LanguageModelV4StreamPart, AISdkError>> = vec![
        Ok(LanguageModelV4StreamPart::TextStart {
            id: "t1".into(),
            provider_metadata: None,
        }),
        Err(AISdkError::new("connection lost")),
    ];

    let mut processor = StreamProcessor::from_stream(Box::pin(stream::iter(items)));

    // First event succeeds.
    assert!(processor.next().await.unwrap().is_ok());

    // Second event is an error.
    let err = processor.next().await.unwrap().unwrap_err();
    assert_eq!(err.message, "connection lost");
}

#[tokio::test]
async fn test_empty_stream_returns_none() {
    let mut processor = processor_from_parts(vec![]);
    assert!(processor.next().await.is_none());
}
