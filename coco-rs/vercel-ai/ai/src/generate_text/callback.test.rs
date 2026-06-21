//! Tests for callback.rs

use super::*;

#[test]
fn test_callback_model_info() {
    let info = CallbackModelInfo::new("anthropic", "claude-3");
    assert_eq!(info.provider, "anthropic");
    assert_eq!(info.model_id, "claude-3");
}

#[test]
fn test_on_start_event() {
    let model = CallbackModelInfo::new("openai", "gpt-4");
    let event = OnStartEvent::new("call-1", model);
    assert_eq!(event.call_id, "call-1");
    assert_eq!(event.model.model_id, "gpt-4");
    assert_eq!(event.model.provider, "openai");
}

#[test]
fn test_on_start_event_builders() {
    let model = CallbackModelInfo::new("openai", "gpt-4");
    let event = OnStartEvent::new("call-1", model)
        .with_system("You are helpful")
        .with_tools(vec!["calc".to_string()])
        .with_tool_choice("auto");

    assert_eq!(event.system, Some("You are helpful".to_string()));
    assert_eq!(event.tools, vec!["calc"]);
    assert_eq!(event.tool_choice, Some("auto".to_string()));
}

#[test]
fn test_on_finish_event() {
    let step_result = StepResult::new(
        0,
        "Hello".to_string(),
        Usage::default(),
        FinishReason::end_turn(),
    );
    let event = OnFinishEvent::new(step_result, Vec::new(), Usage::default());
    assert_eq!(event.text(), "Hello");
    assert_eq!(event.steps.len(), 0);
}

#[test]
fn test_on_step_start_event() {
    let model = CallbackModelInfo::new("openai", "gpt-4");
    let event = OnStepStartEvent::new("call-1", 0, model);
    assert_eq!(event.step_number, 0);
    assert_eq!(event.model.model_id, "gpt-4");
}

#[test]
fn test_on_step_finish_is_step_result() {
    // OnStepFinishEvent is a type alias for StepResult
    let step: OnStepFinishEvent = StepResult::new(
        0,
        "test".to_string(),
        Usage::default(),
        FinishReason::end_turn(),
    );
    assert_eq!(step.step, 0);
    assert_eq!(step.text, "test");
}

#[test]
fn test_tool_call_outcome() {
    let success = ToolCallOutcome::Success {
        output: serde_json::json!({"result": 42}),
    };
    assert!(matches!(success, ToolCallOutcome::Success { .. }));

    let error = ToolCallOutcome::Error {
        error: "failed".to_string(),
    };
    assert!(matches!(error, ToolCallOutcome::Error { .. }));
}

#[test]
fn test_on_tool_call_finish_success() {
    let model = CallbackModelInfo::new("openai", "gpt-4");
    let tc = ToolCall::new("tc-1", "calc", serde_json::json!({}));
    let event = OnToolCallFinishEvent::success("call-1", 0, model, tc, serde_json::json!(42), 100);
    assert!(!event.is_error());
    assert_eq!(event.duration_ms, 100);
}

#[test]
fn test_on_tool_call_finish_error() {
    let model = CallbackModelInfo::new("openai", "gpt-4");
    let tc = ToolCall::new("tc-1", "calc", serde_json::json!({}));
    let event = OnToolCallFinishEvent::error("call-1", 0, model, tc, "failed", 50);
    assert!(event.is_error());
    assert_eq!(event.duration_ms, 50);
}

#[test]
fn test_chunk_event_data() {
    let chunk = OnChunkEvent::text_delta("hello");
    assert!(matches!(chunk.chunk, ChunkEventData::TextDelta { .. }));

    let chunk = OnChunkEvent::reasoning_delta("thinking");
    assert!(matches!(chunk.chunk, ChunkEventData::ReasoningDelta { .. }));

    let chunk = OnChunkEvent::tool_call_start("tc-1", "calc");
    assert!(matches!(chunk.chunk, ChunkEventData::ToolInputStart { .. }));
}

#[test]
fn test_callbacks_builder() {
    let callbacks = GenerateTextCallbacks::new()
        .with_on_start(|_e| println!("Started"))
        .with_on_finish(|_e| println!("Finished"));

    assert!(callbacks.on_start.is_some());
    assert!(callbacks.on_finish.is_some());
}

#[test]
fn test_stream_callbacks_builder() {
    let callbacks = StreamTextCallbacks::new()
        .with_on_start(|_e| println!("Started"))
        .with_on_chunk(|_e| println!("Chunk"))
        .with_on_error(|_e| println!("Error"));

    assert!(callbacks.on_start.is_some());
    assert!(callbacks.on_chunk.is_some());
    assert!(callbacks.on_error.is_some());
}
