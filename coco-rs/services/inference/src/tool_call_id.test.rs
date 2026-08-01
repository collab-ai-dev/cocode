use super::*;
use vercel_ai_provider::LanguageModelV4ToolCall;
use vercel_ai_provider::LanguageModelV4ToolResult;
use vercel_ai_provider::language_model::v4::LanguageModelV4ToolApprovalRequest;

fn provider_call(id: &str, tool_name: &str) -> LanguageModelV4StreamPart {
    LanguageModelV4StreamPart::ToolCall(
        LanguageModelV4ToolCall::new(id, tool_name, "{}").with_provider_executed(true),
    )
}

#[test]
fn late_provider_references_follow_duplicate_calls_in_fifo_order() {
    let mut normalizer = ToolCallIdNormalizer::default();
    let mut first = provider_call("reused", "first");
    let mut second = provider_call("reused", "second");
    normalizer.normalize(&mut first).expect("first call");
    normalizer.normalize(&mut second).expect("second call");

    let mut first_approval = LanguageModelV4StreamPart::ToolApprovalRequest(
        LanguageModelV4ToolApprovalRequest::new("approval-1", "reused"),
    );
    let mut second_approval = LanguageModelV4StreamPart::ToolApprovalRequest(
        LanguageModelV4ToolApprovalRequest::new("approval-2", "reused"),
    );
    normalizer
        .normalize(&mut first_approval)
        .expect("first approval");
    normalizer
        .normalize(&mut second_approval)
        .expect("second approval");

    let LanguageModelV4StreamPart::ToolApprovalRequest(first_approval) = first_approval else {
        panic!("approval request");
    };
    let LanguageModelV4StreamPart::ToolApprovalRequest(second_approval) = second_approval else {
        panic!("approval request");
    };
    assert_eq!(first_approval.tool_call_id, "reused");
    assert_eq!(second_approval.tool_call_id, "reused_d2");

    let mut first_result = LanguageModelV4StreamPart::ToolResult(LanguageModelV4ToolResult::new(
        "reused",
        "first",
        serde_json::json!({"ok": 1}),
    ));
    let mut second_result = LanguageModelV4StreamPart::ToolResult(LanguageModelV4ToolResult::new(
        "reused",
        "second",
        serde_json::json!({"ok": 2}),
    ));
    normalizer
        .normalize(&mut first_result)
        .expect("first result");
    normalizer
        .normalize(&mut second_result)
        .expect("second result");
    let LanguageModelV4StreamPart::ToolResult(first_result) = first_result else {
        panic!("tool result");
    };
    let LanguageModelV4StreamPart::ToolResult(second_result) = second_result else {
        panic!("tool result");
    };
    assert_eq!(first_result.tool_call_id, "reused");
    assert_eq!(second_result.tool_call_id, "reused_d2");
}

#[test]
fn preliminary_results_remain_bound_to_the_same_call() {
    let mut normalizer = ToolCallIdNormalizer::default();
    let mut first = provider_call("reused", "first");
    let mut second = provider_call("reused", "second");
    normalizer.normalize(&mut first).expect("first call");
    normalizer.normalize(&mut second).expect("second call");

    let mut preview = LanguageModelV4ToolResult::new("reused", "first", serde_json::json!({}));
    preview.preliminary = Some(true);
    let mut preview = LanguageModelV4StreamPart::ToolResult(preview);
    let mut final_result = LanguageModelV4StreamPart::ToolResult(LanguageModelV4ToolResult::new(
        "reused",
        "first",
        serde_json::json!({}),
    ));
    let mut next_result = LanguageModelV4StreamPart::ToolResult(LanguageModelV4ToolResult::new(
        "reused",
        "second",
        serde_json::json!({}),
    ));
    normalizer.normalize(&mut preview).expect("preview");
    normalizer.normalize(&mut final_result).expect("final");
    normalizer.normalize(&mut next_result).expect("next");

    for (part, expected) in [
        (preview, "reused"),
        (final_result, "reused"),
        (next_result, "reused_d2"),
    ] {
        let LanguageModelV4StreamPart::ToolResult(result) = part else {
            panic!("tool result");
        };
        assert_eq!(result.tool_call_id, expected);
    }
}

#[test]
fn out_of_order_results_prefer_the_matching_tool_name() {
    let mut normalizer = ToolCallIdNormalizer::default();
    let mut first = provider_call("reused", "first");
    let mut second = provider_call("reused", "second");
    normalizer.normalize(&mut first).expect("first call");
    normalizer.normalize(&mut second).expect("second call");

    let mut second_result = LanguageModelV4StreamPart::ToolResult(LanguageModelV4ToolResult::new(
        "reused",
        "second",
        serde_json::json!({}),
    ));
    let mut first_result = LanguageModelV4StreamPart::ToolResult(LanguageModelV4ToolResult::new(
        "reused",
        "first",
        serde_json::json!({}),
    ));
    normalizer
        .normalize(&mut second_result)
        .expect("second result");
    normalizer
        .normalize(&mut first_result)
        .expect("first result");

    let LanguageModelV4StreamPart::ToolResult(second_result) = second_result else {
        panic!("tool result");
    };
    let LanguageModelV4StreamPart::ToolResult(first_result) = first_result else {
        panic!("tool result");
    };
    assert_eq!(second_result.tool_call_id, "reused_d2");
    assert_eq!(first_result.tool_call_id, "reused");
}

#[test]
fn generated_suffix_never_shadows_a_raw_provider_id() {
    let mut normalizer = ToolCallIdNormalizer::default();
    let mut reserved = provider_call("reused_d2", "reserved");
    let mut first = provider_call("reused", "first");
    let mut second = provider_call("reused", "second");
    normalizer.normalize(&mut reserved).expect("reserved");
    normalizer.normalize(&mut first).expect("first");
    normalizer.normalize(&mut second).expect("second");
    let LanguageModelV4StreamPart::ToolCall(second) = second else {
        panic!("tool call");
    };
    assert_eq!(second.tool_call_id, "reused_d3");
}

#[test]
fn excess_provider_reference_is_rejected_instead_of_rebinding() {
    let mut normalizer = ToolCallIdNormalizer::default();
    let mut call = provider_call("reused", "first");
    normalizer.normalize(&mut call).expect("provider call");
    let mut first = LanguageModelV4StreamPart::ToolResult(LanguageModelV4ToolResult::new(
        "reused",
        "first",
        serde_json::json!({}),
    ));
    normalizer.normalize(&mut first).expect("first result");
    let mut excess = LanguageModelV4StreamPart::ToolResult(LanguageModelV4ToolResult::new(
        "reused",
        "first",
        serde_json::json!({}),
    ));
    assert!(matches!(
        normalizer.normalize(&mut excess),
        Err(ToolCallIdError::UnmatchedReference {
            reference_kind: "tool result",
            ..
        })
    ));
}
