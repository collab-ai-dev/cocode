use super::*;
use serde_json::json;

#[test]
fn test_step_result_new() {
    let step = StepResult::new(
        0,
        "Hello".to_string(),
        Usage::default(),
        FinishReason::end_turn(),
    );

    assert_eq!(step.step, 0);
    assert_eq!(step.text, "Hello");
    assert!(!step.has_tool_calls());
    assert!(step.is_final());
}

#[test]
fn test_step_result_with_model() {
    let step = StepResult::new(
        0,
        "Hello".to_string(),
        Usage::default(),
        FinishReason::end_turn(),
    )
    .with_model(CallbackModelInfo::new("openai", "gpt-4"));

    assert_eq!(step.model.provider, "openai");
    assert_eq!(step.model.model_id, "gpt-4");
    assert_eq!(step.model_id(), "gpt-4");
    assert_eq!(step.provider_id(), "openai");
}

#[test]
fn test_step_result_with_tools() {
    let tool_call = ToolCall::new("id_1", "test_tool", json!({}));
    let tool_result = ToolResult::new("id_1", "test_tool", json!({}));

    let step = StepResult::new(
        0,
        "".to_string(),
        Usage::default(),
        FinishReason::tool_use(),
    )
    .with_tool_calls(vec![tool_call])
    .with_tool_results(vec![tool_result]);

    assert!(step.has_tool_calls());
    assert!(step.has_tool_results());
    assert!(!step.is_final());
}

#[test]
fn test_step_result_error() {
    let step = StepResult::error(0, "Something went wrong");

    assert!(step.is_error);
    assert_eq!(step.error_message, Some("Something went wrong".to_string()));
}

#[test]
fn test_step_result_from_content() {
    let content = vec![
        AssistantContentPart::text("Hello"),
        AssistantContentPart::text(" world"),
    ];

    let step = StepResult::from_content(0, content, Usage::default(), FinishReason::end_turn());

    assert_eq!(step.text, "Hello world");
}

#[test]
fn test_step_result_message_types() {
    let tool_call = ToolCall::new("id_1", "test", json!({}));
    let tool_result = ToolResult::new("id_1", "test", json!({}));

    let step = StepResult::new(
        0,
        "Result:".to_string(),
        Usage::default(),
        FinishReason::tool_use(),
    )
    .with_tool_calls(vec![tool_call])
    .with_tool_results(vec![tool_result]);

    let types = step.message_types();
    assert!(types.contains(&"text"));
    assert!(types.contains(&"tool_calls"));
    assert!(types.contains(&"tool_results"));
}

#[test]
fn test_step_result_telemetry_fields() {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("key".to_string(), json!("value"));

    let step = StepResult::new(
        0,
        "Hello".to_string(),
        Usage::default(),
        FinishReason::end_turn(),
    )
    .with_function_id("my-function")
    .with_metadata(metadata.clone())
    .with_raw_finish_reason("stop");

    assert_eq!(step.function_id, Some("my-function".to_string()));
    assert_eq!(step.raw_finish_reason, Some("stop".to_string()));
    assert!(step.metadata.is_some());
    assert_eq!(step.metadata.unwrap()["key"], json!("value"));
}

#[test]
fn test_step_result_convenience_setters() {
    let step = StepResult::new(
        0,
        "Hello".to_string(),
        Usage::default(),
        FinishReason::end_turn(),
    )
    .with_model_id("gpt-4")
    .with_provider_id("openai");

    assert_eq!(step.model.model_id, "gpt-4");
    assert_eq!(step.model.provider, "openai");
}

#[test]
fn test_step_result_experimental_context() {
    let step = StepResult::new(
        0,
        "Hello".to_string(),
        Usage::default(),
        FinishReason::end_turn(),
    )
    .with_experimental_context(json!({"key": "value"}));

    assert_eq!(step.experimental_context, Some(json!({"key": "value"})));
}

#[test]
fn test_step_result_no_experimental_context() {
    let step = StepResult::new(
        0,
        "Hello".to_string(),
        Usage::default(),
        FinishReason::end_turn(),
    );

    assert!(step.experimental_context.is_none());
}
