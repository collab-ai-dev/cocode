use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;
use futures::StreamExt;
use vercel_ai_provider::AISdkError;
use vercel_ai_provider::FinishReason;
use vercel_ai_provider::LanguageModelV4StreamPart;
use vercel_ai_provider::LanguageModelV4StreamResult;
use vercel_ai_provider::Usage;

use super::*;
use crate::model::LanguageModel;
use crate::prompt::Prompt;
use crate::test_utils::MockLanguageModel;

fn json_schema() -> vercel_ai_provider::JSONSchema {
    serde_json::json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "age": { "type": "integer" }
        },
        "required": ["name", "age"]
    })
}

fn make_stream_model(json_text: &str) -> Arc<MockLanguageModel> {
    let text = json_text.to_string();
    Arc::new(
        MockLanguageModel::builder()
            .with_stream_handler(move |_| {
                let text = text.clone();
                let parts = vec![
                    Ok(LanguageModelV4StreamPart::TextDelta {
                        id: String::new(),
                        delta: text,
                        provider_metadata: None,
                    }),
                    Ok(LanguageModelV4StreamPart::Finish {
                        finish_reason: FinishReason::end_turn(),
                        usage: Usage::new(10, 5),
                        provider_metadata: None,
                    }),
                ];
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
    )
}

#[derive(Debug, serde::Deserialize, PartialEq)]
struct Person {
    name: String,
    age: u32,
}

#[tokio::test]
async fn test_stream_object_into_object() {
    let model = make_stream_model(r#"{"name":"Alice","age":30}"#);
    let options = StreamObjectOptions::<Person>::new(
        LanguageModel::from_v4(model),
        Prompt::user("Generate a person"),
        json_schema(),
    );

    let result = stream_object(options);
    let person = result.into_object().await.unwrap();
    assert_eq!(person.name, "Alice");
    assert_eq!(person.age, 30);
}

#[tokio::test]
async fn test_stream_object_text_stream() {
    let model = make_stream_model(r#"{"name":"Bob","age":25}"#);
    let options = StreamObjectOptions::<Person>::new(
        LanguageModel::from_v4(model),
        Prompt::user("Generate a person"),
        json_schema(),
    );

    let result = stream_object(options);
    let text_parts: Vec<String> = result.text_stream().collect().await;
    let full_text = text_parts.join("");
    assert_eq!(full_text, r#"{"name":"Bob","age":25}"#);
}

#[tokio::test]
async fn test_stream_object_partial_object_stream() {
    // Send JSON in two chunks to trigger partial object emissions
    let model = Arc::new(
        MockLanguageModel::builder()
            .with_stream_handler(|_| {
                let parts = vec![
                    Ok(LanguageModelV4StreamPart::TextDelta {
                        id: String::new(),
                        delta: r#"{"name":"Carol""#.to_string(),
                        provider_metadata: None,
                    }),
                    Ok(LanguageModelV4StreamPart::TextDelta {
                        id: String::new(),
                        delta: r#","age":42}"#.to_string(),
                        provider_metadata: None,
                    }),
                    Ok(LanguageModelV4StreamPart::Finish {
                        finish_reason: FinishReason::end_turn(),
                        usage: Usage::new(10, 5),
                        provider_metadata: None,
                    }),
                ];
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

    let options = StreamObjectOptions::<Person>::new(
        LanguageModel::from_v4(model),
        Prompt::user("Generate a person"),
        json_schema(),
    );

    let result = stream_object(options);
    let partials: Vec<serde_json::Value> = result.partial_object_stream().collect().await;

    // Should have at least one partial object
    assert!(!partials.is_empty());
    // The last partial should be the complete object
    let last = partials.last().unwrap();
    assert_eq!(last["name"], "Carol");
    assert_eq!(last["age"], 42);
}

#[tokio::test]
async fn test_stream_object_invalid_json_error() {
    let model = make_stream_model("not valid json");
    let options = StreamObjectOptions::<Person>::new(
        LanguageModel::from_v4(model),
        Prompt::user("Generate a person"),
        json_schema(),
    );

    let result = stream_object(options);
    let err = result.into_object().await.unwrap_err();
    assert!(matches!(err, AIError::SchemaValidation(_)));
}

#[tokio::test]
async fn test_stream_object_options_builder() {
    let options =
        StreamObjectOptions::<Person>::new("test-model", Prompt::user("Hi"), json_schema())
            .with_schema_name("person")
            .with_schema_description("A person")
            .with_mode(ObjectGenerationMode::Tool);

    assert_eq!(options.schema_name, Some("person".to_string()));
    assert_eq!(options.schema_description, Some("A person".to_string()));
    assert_eq!(options.mode, ObjectGenerationMode::Tool);
}

#[tokio::test]
async fn test_stream_object_usage_accessor() {
    let model = make_stream_model(r#"{"name":"Alice","age":30}"#);
    let options = StreamObjectOptions::<Person>::new(
        LanguageModel::from_v4(model),
        Prompt::user("Generate a person"),
        json_schema(),
    );

    let mut result = stream_object(options);
    // Consume the stream first
    while result.stream.next().await.is_some() {}
    // Then access usage via the watch channel
    let usage = result.usage().await;
    assert_eq!(usage.input_tokens.total(), Some(10));
    assert_eq!(usage.output_tokens.total, Some(5));
}

#[tokio::test]
async fn test_stream_object_finish_reason_accessor() {
    let model = make_stream_model(r#"{"name":"Alice","age":30}"#);
    let options = StreamObjectOptions::<Person>::new(
        LanguageModel::from_v4(model),
        Prompt::user("Generate a person"),
        json_schema(),
    );

    let mut result = stream_object(options);
    while result.stream.next().await.is_some() {}
    let fr = result.finish_reason().await;
    assert!(fr.is_stop());
}

#[tokio::test]
async fn test_stream_object_warnings_accessor() {
    let model = make_stream_model(r#"{"name":"Alice","age":30}"#);
    let options = StreamObjectOptions::<Person>::new(
        LanguageModel::from_v4(model),
        Prompt::user("Generate a person"),
        json_schema(),
    );

    let mut result = stream_object(options);
    while result.stream.next().await.is_some() {}
    let warnings = result.warnings().await;
    assert!(warnings.is_empty());
}

#[tokio::test]
async fn test_stream_object_on_finish_with_object_json() {
    let model = make_stream_model(r#"{"name":"Dave","age":28}"#);
    let finish_data: Arc<std::sync::Mutex<Option<StreamObjectFinishEvent>>> =
        Arc::new(std::sync::Mutex::new(None));
    let finish_data_clone = finish_data.clone();

    let options = StreamObjectOptions::<Person>::new(
        LanguageModel::from_v4(model),
        Prompt::user("Generate a person"),
        json_schema(),
    )
    .with_on_finish(move |event: &StreamObjectFinishEvent| {
        *finish_data_clone.lock().unwrap() = Some(event.clone());
    });

    let result = stream_object(options);
    let _ = result.into_object().await.unwrap();

    // Give time for the callback to fire
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let data = finish_data.lock().unwrap();
    let event = data.as_ref().expect("on_finish should have been called");
    assert!(event.object_json.is_some());
    assert!(event.error.is_none());
    assert_eq!(event.object_json.as_ref().unwrap()["name"], "Dave");
}

#[tokio::test]
async fn test_stream_object_on_finish_with_error() {
    let model = make_stream_model("not valid json");
    let finish_data: Arc<std::sync::Mutex<Option<StreamObjectFinishEvent>>> =
        Arc::new(std::sync::Mutex::new(None));
    let finish_data_clone = finish_data.clone();

    let options = StreamObjectOptions::<Person>::new(
        LanguageModel::from_v4(model),
        Prompt::user("Generate a person"),
        json_schema(),
    )
    .with_on_finish(move |event: &StreamObjectFinishEvent| {
        *finish_data_clone.lock().unwrap() = Some(event.clone());
    });

    let result = stream_object(options);
    let _ = result.into_object().await;

    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let data = finish_data.lock().unwrap();
    let event = data
        .as_ref()
        .expect("on_finish should have been called on error");
    assert!(event.object_json.is_none());
    assert!(event.error.is_some());
    assert!(
        event
            .error
            .as_ref()
            .unwrap()
            .contains("Failed to parse JSON")
    );
}

#[tokio::test]
async fn test_stream_object_object_accessor() {
    let model = make_stream_model(r#"{"name":"Eve","age":22}"#);
    let options = StreamObjectOptions::<Person>::new(
        LanguageModel::from_v4(model),
        Prompt::user("Generate a person"),
        json_schema(),
    );

    let mut result = stream_object(options);
    let person = result.object().await.unwrap();
    assert_eq!(person.name, "Eve");
    assert_eq!(person.age, 22);
}

#[tokio::test]
async fn test_stream_object_response_metadata() {
    // Build a model that provides response metadata with headers
    let model = Arc::new(
        MockLanguageModel::builder()
            .with_stream_handler(|_| {
                let parts = vec![
                    Ok(LanguageModelV4StreamPart::TextDelta {
                        id: String::new(),
                        delta: r#"{"name":"Frank","age":40}"#.to_string(),
                        provider_metadata: None,
                    }),
                    Ok(LanguageModelV4StreamPart::Finish {
                        finish_reason: FinishReason::end_turn(),
                        usage: Usage::new(10, 5),
                        provider_metadata: None,
                    }),
                ];
                let stream: Pin<
                    Box<dyn Stream<Item = Result<LanguageModelV4StreamPart, AISdkError>> + Send>,
                > = Box::pin(futures::stream::iter(parts));

                let mut headers = std::collections::HashMap::new();
                headers.insert("x-request-id".to_string(), "abc-123".to_string());

                Ok(LanguageModelV4StreamResult {
                    stream,
                    request: None,
                    response: Some(
                        vercel_ai_provider::LanguageModelV4StreamResponse::new()
                            .with_headers(headers),
                    ),
                })
            })
            .build(),
    );

    let options = StreamObjectOptions::<Person>::new(
        LanguageModel::from_v4(model),
        Prompt::user("Generate a person"),
        json_schema(),
    );

    let mut result = stream_object(options);
    // Consume the stream
    while result.stream.next().await.is_some() {}
    // Check response metadata
    let resp = result.response().await;
    assert!(resp.is_some());
    let resp = resp.unwrap();
    assert_eq!(
        resp.headers.as_ref().unwrap().get("x-request-id").unwrap(),
        "abc-123"
    );
}

#[tokio::test]
async fn test_stream_object_provider_metadata_from_finish() {
    // Verify that provider_metadata from the Finish stream part is propagated
    let model = Arc::new(
        MockLanguageModel::builder()
            .with_stream_handler(|_| {
                let mut pm = vercel_ai_provider::ProviderMetadata::new();
                pm.set(
                    "anthropic",
                    serde_json::json!({"model_version": "2024-01-01"}),
                );

                let parts = vec![
                    Ok(LanguageModelV4StreamPart::TextDelta {
                        id: String::new(),
                        delta: r#"{"name":"Grace","age":35}"#.to_string(),
                        provider_metadata: None,
                    }),
                    Ok(LanguageModelV4StreamPart::Finish {
                        finish_reason: FinishReason::end_turn(),
                        usage: Usage::new(10, 5),
                        provider_metadata: Some(pm),
                    }),
                ];
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

    let options = StreamObjectOptions::<Person>::new(
        LanguageModel::from_v4(model),
        Prompt::user("Generate a person"),
        json_schema(),
    );

    let mut result = stream_object(options);
    while result.stream.next().await.is_some() {}
    let pm = result.provider_metadata().await;
    assert!(pm.is_some());
    let pm = pm.unwrap();
    let anthropic = pm.get("anthropic").expect("should have anthropic metadata");
    assert_eq!(anthropic["model_version"], "2024-01-01");
}

#[tokio::test]
async fn test_stream_object_no_double_error_on_provider_error() {
    // Verify that a provider stream error only produces one Error event, not two
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

    let options = StreamObjectOptions::<Person>::new(
        LanguageModel::from_v4(model),
        Prompt::user("Generate a person"),
        json_schema(),
    );

    let mut result = stream_object(options);
    let mut error_count = 0;
    while let Some(part) = result.stream.next().await {
        if matches!(part, ObjectStreamPart::Error { .. }) {
            error_count += 1;
        }
    }
    // Should only see exactly one error (not two)
    assert_eq!(error_count, 1);
}
