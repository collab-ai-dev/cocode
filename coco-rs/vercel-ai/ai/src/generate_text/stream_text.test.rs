use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use futures::Stream;
use futures::StreamExt;
use vercel_ai_provider::AISdkError;
use vercel_ai_provider::LanguageModelV4StreamPart;
use vercel_ai_provider::LanguageModelV4StreamResult;

use super::*;
use crate::model::LanguageModel;
use crate::prompt::Prompt;
use crate::test_utils::MockLanguageModel;
use crate::types::SimpleTool;
use crate::types::ToolRegistry;

#[tokio::test]
async fn test_stream_text_no_double_error_on_provider_error() {
    // Verify that a provider stream error only produces one Error event, not two.
    // This mirrors test_stream_object_no_double_error_on_provider_error.
    let model = Arc::new(
        MockLanguageModel::builder()
            .with_stream_handler(|_| {
                let parts: Vec<Result<LanguageModelV4StreamPart, AISdkError>> =
                    vec![Err(AISdkError::new("provider failure"))];
                let stream: Pin<
                    Box<dyn Stream<Item = Result<LanguageModelV4StreamPart, AISdkError>> + Send>,
                > = Box::pin(futures::stream::iter(parts));

                Ok(LanguageModelV4StreamResult {
                    stream,
                    request: None,
                    response: None,
                })
            })
            .build(),
    );

    let options = StreamTextOptions::new(LanguageModel::from_v4(model), Prompt::user("Hello"));

    let result = stream_text(options);
    let mut error_count = 0;
    let mut stream = result.stream;
    while let Some(part) = stream.next().await {
        if matches!(part, TextStreamPart::Error { .. }) {
            error_count += 1;
        }
    }
    // Should only see exactly one error (not two)
    assert_eq!(error_count, 1);
}

#[tokio::test]
async fn test_stream_text_passes_tool_context() {
    let observed_tool_context = Arc::new(Mutex::new(None::<String>));
    let observed_tool_context_clone = observed_tool_context.clone();
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(SimpleTool::with_name("lookup").handler(
        move |_input, options| {
            let observed_tool_context = observed_tool_context_clone.clone();
            async move {
                let context = options
                    .get_context::<String>()
                    .expect("tool context should be provided");
                *observed_tool_context.lock().expect("lock tool context") = Some(context.clone());
                Ok(serde_json::json!({ "context": context }))
            }
        },
    )));

    let model = Arc::new(
        MockLanguageModel::builder()
            .with_stream_tool_call_response("call-1", "lookup", serde_json::json!({}))
            .build(),
    );

    let options = StreamTextOptions::new(LanguageModel::from_v4(model), Prompt::user("Hello"))
        .with_tools(Arc::new(registry))
        .with_tool_context("lookup", "stream-tool-context".to_string());

    let result = stream_text(options);
    let mut stream = result.stream;
    while stream.next().await.is_some() {}

    assert_eq!(
        *observed_tool_context.lock().expect("lock tool context"),
        Some("stream-tool-context".to_string())
    );
}
