use super::*;
use vercel_ai_provider::LanguageModelV4ToolCall;
use vercel_ai_provider::LanguageModelV4ToolResult;
use vercel_ai_provider::language_model::v4::LanguageModelV4ToolApprovalRequest;

fn tool_call(id: &str, tool_name: &str) -> LanguageModelV4StreamPart {
    LanguageModelV4StreamPart::ToolCall(LanguageModelV4ToolCall::new(id, tool_name, "{}"))
}

fn input_start(id: &str, tool_name: &str) -> LanguageModelV4StreamPart {
    LanguageModelV4StreamPart::ToolInputStart {
        id: id.to_string(),
        tool_name: tool_name.to_string(),
        title: None,
        provider_executed: None,
        dynamic: None,
        provider_metadata: None,
    }
}

fn call_id_of(part: &LanguageModelV4StreamPart) -> &str {
    match part {
        LanguageModelV4StreamPart::ToolCall(call) => &call.tool_call_id,
        LanguageModelV4StreamPart::ToolResult(result) => &result.tool_call_id,
        LanguageModelV4StreamPart::ToolApprovalRequest(request) => &request.tool_call_id,
        LanguageModelV4StreamPart::ToolInputStart { id, .. }
        | LanguageModelV4StreamPart::ToolInputDelta { id, .. }
        | LanguageModelV4StreamPart::ToolInputEnd { id, .. } => id,
        other => panic!("part carries no tool call id: {other:?}"),
    }
}

#[test]
fn sequential_reuse_of_one_id_gets_a_deterministic_suffix() {
    let mut normalizer = ToolCallIdNormalizer::default();
    let mut first = tool_call("reused", "first");
    let mut second = tool_call("reused", "second");
    let mut third = tool_call("reused", "third");
    normalizer.normalize(&mut first).expect("first call");
    normalizer.normalize(&mut second).expect("second call");
    normalizer.normalize(&mut third).expect("third call");

    assert_eq!(call_id_of(&first), "reused");
    assert_eq!(call_id_of(&second), "reused_d2");
    assert_eq!(call_id_of(&third), "reused_d3");
}

#[test]
fn streamed_input_parts_follow_their_call_to_the_effective_id() {
    let mut normalizer = ToolCallIdNormalizer::default();
    let mut first_start = input_start("reused", "first");
    normalizer.normalize(&mut first_start).expect("first start");
    let mut first_call = tool_call("reused", "first");
    normalizer.normalize(&mut first_call).expect("first call");

    // Second call reuses the same wire id; its deltas must land on the alias.
    let mut second_start = input_start("reused", "second");
    normalizer
        .normalize(&mut second_start)
        .expect("second start");
    let mut delta = LanguageModelV4StreamPart::ToolInputDelta {
        id: "reused".to_string(),
        delta: "{}".to_string(),
        provider_metadata: None,
    };
    normalizer.normalize(&mut delta).expect("delta");

    assert_eq!(call_id_of(&first_start), "reused");
    assert_eq!(call_id_of(&first_call), "reused");
    assert_eq!(call_id_of(&second_start), "reused_d2");
    assert_eq!(call_id_of(&delta), "reused_d2");
}

#[test]
fn generated_suffix_never_shadows_a_raw_provider_id() {
    let mut normalizer = ToolCallIdNormalizer::default();
    let mut reserved = tool_call("reused_d2", "reserved");
    let mut first = tool_call("reused", "first");
    let mut second = tool_call("reused", "second");
    normalizer.normalize(&mut reserved).expect("reserved");
    normalizer.normalize(&mut first).expect("first");
    normalizer.normalize(&mut second).expect("second");
    assert_eq!(call_id_of(&second), "reused_d3");
}

#[test]
fn overlapping_reuse_of_a_still_open_id_is_the_one_fatal_case() {
    let mut normalizer = ToolCallIdNormalizer::default();
    let mut first = input_start("reused", "first");
    normalizer.normalize(&mut first).expect("first start");
    // No `ToolCall` closed the first block, so deltas for a second block with
    // the same id would be unattributable.
    let mut overlapping = input_start("reused", "second");
    let error = normalizer
        .normalize(&mut overlapping)
        .expect_err("overlapping start");
    assert!(error.to_string().contains("overlapping tool calls"));
}

/// Provider-executed result / approval parts are dropped downstream, so this
/// seam deliberately leaves their ids alone rather than maintaining a binding
/// nobody reads. Pinning it here so a future reader doesn't "fix" the omission
/// without first wiring those parts into assistant content.
#[test]
fn result_and_approval_ids_are_left_verbatim() {
    let mut normalizer = ToolCallIdNormalizer::default();
    let mut first = tool_call("reused", "first");
    let mut second = tool_call("reused", "second");
    normalizer.normalize(&mut first).expect("first call");
    normalizer.normalize(&mut second).expect("second call");
    assert_eq!(call_id_of(&second), "reused_d2");

    let mut result = LanguageModelV4StreamPart::ToolResult(LanguageModelV4ToolResult::new(
        "reused",
        "second",
        serde_json::json!({}),
    ));
    let mut approval = LanguageModelV4StreamPart::ToolApprovalRequest(
        LanguageModelV4ToolApprovalRequest::new("approval-1", "reused"),
    );
    normalizer.normalize(&mut result).expect("result");
    normalizer.normalize(&mut approval).expect("approval");

    assert_eq!(call_id_of(&result), "reused");
    assert_eq!(call_id_of(&approval), "reused");
}
