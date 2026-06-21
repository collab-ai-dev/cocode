use std::sync::Arc;
use std::sync::Mutex;

use serde::Deserialize;
use serde::Serialize;

use super::*;
use crate::error::AIError;
use crate::model::LanguageModel;
use crate::prompt::Prompt;
use crate::test_utils::MockLanguageModel;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct TestPerson {
    name: String,
    age: u32,
}

fn person_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "age": { "type": "integer" }
        },
        "required": ["name", "age"]
    })
}

#[tokio::test]
async fn test_generate_object_json_mode() {
    let model = MockLanguageModel::builder()
        .with_text_response(r#"{"name":"Alice","age":30}"#)
        .build();

    let options = GenerateObjectOptions::<TestPerson>::new(
        LanguageModel::from_v4(Arc::new(model)),
        Prompt::user("Generate a person"),
        person_schema(),
    );

    let result = generate_object(options).await.unwrap();
    assert_eq!(result.object.name, "Alice");
    assert_eq!(result.object.age, 30);
    assert_eq!(result.raw, r#"{"name":"Alice","age":30}"#);
}

#[tokio::test]
async fn test_generate_object_tool_mode() {
    let model = MockLanguageModel::builder()
        .with_tool_call_response(
            "call_1",
            "json_output",
            serde_json::json!({"name": "Bob", "age": 25}),
        )
        .build();

    let options = GenerateObjectOptions::<TestPerson>::new(
        LanguageModel::from_v4(Arc::new(model)),
        Prompt::user("Generate a person"),
        person_schema(),
    )
    .with_mode(ObjectGenerationMode::Tool);

    let result = generate_object(options).await.unwrap();
    assert_eq!(result.object.name, "Bob");
    assert_eq!(result.object.age, 25);
}

#[tokio::test]
async fn test_generate_object_schema_name_description() {
    let model = MockLanguageModel::builder()
        .with_tool_call_response(
            "call_1",
            "person",
            serde_json::json!({"name": "Charlie", "age": 40}),
        )
        .build();

    let options = GenerateObjectOptions::<TestPerson>::new(
        LanguageModel::from_v4(Arc::new(model)),
        Prompt::user("Generate a person"),
        person_schema(),
    )
    .with_mode(ObjectGenerationMode::Tool)
    .with_schema_name("person")
    .with_schema_description("A person object");

    let result = generate_object(options).await.unwrap();
    assert_eq!(result.object.name, "Charlie");
}

#[tokio::test]
async fn test_generate_object_usage_propagation() {
    let model = MockLanguageModel::builder()
        .with_text_response(r#"{"name":"Test","age":1}"#)
        .build();

    let options = GenerateObjectOptions::<TestPerson>::new(
        LanguageModel::from_v4(Arc::new(model)),
        Prompt::user("Generate"),
        person_schema(),
    );

    let result = generate_object(options).await.unwrap();
    // MockLanguageModel default usage is Usage::new(10, 5)
    assert_eq!(result.usage.total_input_tokens(), 10);
    assert_eq!(result.usage.total_output_tokens(), 5);
}

#[tokio::test]
async fn test_generate_object_finish_reason() {
    let model = MockLanguageModel::builder()
        .with_text_response(r#"{"name":"Test","age":1}"#)
        .build();

    let options = GenerateObjectOptions::<TestPerson>::new(
        LanguageModel::from_v4(Arc::new(model)),
        Prompt::user("Generate"),
        person_schema(),
    );

    let result = generate_object(options).await.unwrap();
    assert!(result.finish_reason.is_stop());
}

#[tokio::test]
async fn test_generate_object_on_finish_callback() {
    let finish_data: Arc<Mutex<Option<GenerateObjectFinishEvent>>> = Arc::new(Mutex::new(None));
    let finish_data_clone = finish_data.clone();

    let model = MockLanguageModel::builder()
        .with_text_response(r#"{"name":"Test","age":1}"#)
        .build();

    let options = GenerateObjectOptions::<TestPerson>::new(
        LanguageModel::from_v4(Arc::new(model)),
        Prompt::user("Generate"),
        person_schema(),
    )
    .with_on_finish(move |event: &GenerateObjectFinishEvent| {
        *finish_data_clone.lock().unwrap() = Some(event.clone());
    });

    let _ = generate_object(options).await.unwrap();

    let finish = finish_data
        .lock()
        .unwrap()
        .take()
        .expect("on_finish should have been called");
    assert_eq!(finish.raw, r#"{"name":"Test","age":1}"#);
    assert!(finish.finish_reason.is_stop());
}

#[tokio::test]
async fn test_generate_object_repair_text() {
    // Model returns invalid JSON
    let model = MockLanguageModel::builder()
        .with_text_response(r#"{"name":"Test","age":1,}"#) // trailing comma
        .build();

    let options = GenerateObjectOptions::<TestPerson>::new(
        LanguageModel::from_v4(Arc::new(model)),
        Prompt::user("Generate"),
        person_schema(),
    )
    .with_repair_text(|_text: &str, _error: &str| Ok(r#"{"name":"Test","age":1}"#.to_string()));

    let result = generate_object(options).await.unwrap();
    assert_eq!(result.object.name, "Test");
    assert_eq!(result.object.age, 1);
}

#[tokio::test]
async fn test_generate_object_invalid_json_error() {
    let model = MockLanguageModel::builder()
        .with_text_response("not json at all")
        .build();

    let options = GenerateObjectOptions::<TestPerson>::new(
        LanguageModel::from_v4(Arc::new(model)),
        Prompt::user("Generate"),
        person_schema(),
    );

    let result = generate_object(options).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        AIError::SchemaValidation(msg) => {
            assert!(msg.contains("Failed to parse JSON"));
        }
        other => panic!("Expected SchemaValidation error, got: {other}"),
    }
}

#[tokio::test]
async fn test_generate_object_warning_propagation() {
    use vercel_ai_provider::AssistantContentPart;
    use vercel_ai_provider::FinishReason;
    use vercel_ai_provider::LanguageModelV4GenerateResult;
    use vercel_ai_provider::Usage;
    use vercel_ai_provider::Warning;

    let model = MockLanguageModel::builder()
        .with_generate_handler(|_| {
            Ok(LanguageModelV4GenerateResult {
                content: vec![AssistantContentPart::text(r#"{"name":"W","age":1}"#)],
                usage: Usage::new(10, 5),
                finish_reason: FinishReason::end_turn(),
                warnings: vec![Warning::other("test warning")],
                provider_metadata: None,
                request: None,
                response: None,
            })
        })
        .build();

    let options = GenerateObjectOptions::<TestPerson>::new(
        LanguageModel::from_v4(Arc::new(model)),
        Prompt::user("Generate"),
        person_schema(),
    );

    let result = generate_object(options).await.unwrap();
    assert_eq!(result.warnings.len(), 1);
}

#[tokio::test]
async fn test_generate_object_response_metadata() {
    use vercel_ai_provider::AssistantContentPart;
    use vercel_ai_provider::FinishReason;
    use vercel_ai_provider::LanguageModelV4GenerateResult;
    use vercel_ai_provider::LanguageModelV4Response;
    use vercel_ai_provider::Usage;

    let model = MockLanguageModel::builder()
        .with_generate_handler(|_| {
            Ok(LanguageModelV4GenerateResult {
                content: vec![AssistantContentPart::text(r#"{"name":"R","age":1}"#)],
                usage: Usage::new(10, 5),
                finish_reason: FinishReason::end_turn(),
                warnings: Vec::new(),
                provider_metadata: None,
                request: None,
                response: Some(LanguageModelV4Response::new().with_model_id("test-model")),
            })
        })
        .build();

    let options = GenerateObjectOptions::<TestPerson>::new(
        LanguageModel::from_v4(Arc::new(model)),
        Prompt::user("Generate"),
        person_schema(),
    );

    let result = generate_object(options).await.unwrap();
    assert!(result.response.is_some());
    let resp = result.response.unwrap();
    assert_eq!(resp.model_id, Some("test-model".to_string()));
}
