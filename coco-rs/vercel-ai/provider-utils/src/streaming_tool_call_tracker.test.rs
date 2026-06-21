use super::*;
use vercel_ai_provider::LanguageModelV4StreamPart;

fn make_first_delta(index: usize, id: &str, name: &str, args: &str) -> StreamingToolCallDelta {
    StreamingToolCallDelta {
        index: Some(index),
        id: Some(id.to_string()),
        r#type: Some("function".to_string()),
        function: Some(ToolCallDeltaFunction {
            name: Some(name.to_string()),
            arguments: Some(args.to_string()),
        }),
        extra: None,
    }
}

fn make_args_delta(index: usize, args: &str) -> StreamingToolCallDelta {
    StreamingToolCallDelta {
        index: Some(index),
        id: None,
        r#type: None,
        function: Some(ToolCallDeltaFunction {
            name: None,
            arguments: Some(args.to_string()),
        }),
        extra: None,
    }
}

#[test]
fn single_chunk_complete_tool_call() {
    let mut tracker = StreamingToolCallTracker::new();
    tracker
        .process_delta(make_first_delta(0, "call_1", "search", r#"{"q":"hi"}"#))
        .unwrap();

    // Single-chunk complete call: ToolInputStart + ToolInputDelta + ToolInputEnd + ToolCall = 4
    let parts = tracker.take_parts();
    assert_eq!(parts.len(), 4);
    assert!(matches!(
        parts[0],
        LanguageModelV4StreamPart::ToolInputStart { .. }
    ));
    assert!(matches!(
        parts[1],
        LanguageModelV4StreamPart::ToolInputDelta { .. }
    ));
    assert!(matches!(
        parts[2],
        LanguageModelV4StreamPart::ToolInputEnd { .. }
    ));
    assert!(matches!(parts[3], LanguageModelV4StreamPart::ToolCall(_)));
}

#[test]
fn multi_chunk_tool_call_completes_when_parsable() {
    let mut tracker = StreamingToolCallTracker::new();
    // First chunk: partial JSON, not yet parsable
    tracker
        .process_delta(make_first_delta(0, "call_2", "weather", r#"{"city":"#))
        .unwrap();

    let parts = tracker.take_parts();
    // ToolInputStart + ToolInputDelta (partial args) — no ToolCall yet
    let has_tool_call = parts
        .iter()
        .any(|p| matches!(p, LanguageModelV4StreamPart::ToolCall(_)));
    assert!(
        !has_tool_call,
        "Should not have ToolCall yet for partial args"
    );

    // Second chunk completes the JSON — tracker should auto-finish
    tracker
        .process_delta(make_args_delta(0, r#""NYC"}"#))
        .unwrap();

    let parts = tracker.take_parts();
    assert!(
        parts
            .iter()
            .any(|p| matches!(p, LanguageModelV4StreamPart::ToolCall(_))),
        "Should have ToolCall after args become parsable"
    );
}

#[test]
fn flush_finalizes_incomplete_tool_call() {
    let mut tracker = StreamingToolCallTracker::new();
    // Partial JSON that never completes on its own
    tracker
        .process_delta(make_first_delta(0, "call_3", "search", r#"{"q":"test"#))
        .unwrap();
    tracker.take_parts(); // drain

    // Still incomplete — flush should complete it
    tracker.flush();
    let parts = tracker.take_parts();
    assert!(
        parts
            .iter()
            .any(|p| matches!(p, LanguageModelV4StreamPart::ToolCall(_))),
        "Flush should emit ToolCall for incomplete tool call"
    );
}

#[test]
fn missing_id_returns_error() {
    let mut tracker = StreamingToolCallTracker::new();
    let result = tracker.process_delta(StreamingToolCallDelta {
        index: Some(0),
        id: None,
        r#type: Some("function".to_string()),
        function: Some(ToolCallDeltaFunction {
            name: Some("search".to_string()),
            arguments: Some("{}".to_string()),
        }),
        extra: None,
    });
    assert!(result.is_err());
}

#[test]
fn type_validation_required_rejects_non_function() {
    let mut tracker = StreamingToolCallTracker::with_options(StreamingToolCallTrackerOptions {
        type_validation: TypeValidation::Required,
        ..Default::default()
    });
    let result = tracker.process_delta(StreamingToolCallDelta {
        index: Some(0),
        id: Some("id".to_string()),
        r#type: Some("other".to_string()),
        function: Some(ToolCallDeltaFunction {
            name: Some("tool".to_string()),
            arguments: Some("{}".to_string()),
        }),
        extra: None,
    });
    assert!(result.is_err());
}

#[test]
fn type_validation_if_present_accepts_missing_type() {
    let mut tracker = StreamingToolCallTracker::with_options(StreamingToolCallTrackerOptions {
        type_validation: TypeValidation::IfPresent,
        ..Default::default()
    });
    let result = tracker.process_delta(StreamingToolCallDelta {
        index: Some(0),
        id: Some("id".to_string()),
        r#type: None,
        function: Some(ToolCallDeltaFunction {
            name: Some("tool".to_string()),
            arguments: Some("{}".to_string()),
        }),
        extra: None,
    });
    assert!(result.is_ok());
}
