use coco_config::{LoopGuardrailConfig, LoopGuardrailLevel};
use coco_types::{ToolId, ToolName};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

fn config(level: LoopGuardrailLevel) -> LoopGuardrailConfig {
    LoopGuardrailConfig {
        level,
        ..LoopGuardrailConfig::default()
    }
}

fn read_tool() -> ToolId {
    ToolId::Builtin(ToolName::Read)
}

fn bash_tool() -> ToolId {
    ToolId::Builtin(ToolName::Bash)
}

#[test]
fn off_level_yields_no_handle() {
    assert!(LoopGuardrailHandle::from_config(&config(LoopGuardrailLevel::Off)).is_none());
    assert!(LoopGuardrailHandle::from_config(&config(LoopGuardrailLevel::WarnOnly)).is_some());
}

#[test]
fn exact_failure_warns_at_threshold() {
    let guard = LoopGuardrailHandle::from_config(&config(LoopGuardrailLevel::WarnOnly))
        .expect("handle expected");
    let args = json!({"file_path": "/tmp/x.rs"});
    // 1st failure: below warn_after=2 → no warning.
    assert!(
        guard
            .record_after_call(&read_tool(), &args, true, false, None)
            .is_none()
    );
    // 2nd identical failure: warn.
    let warning = guard
        .record_after_call(&read_tool(), &args, true, false, None)
        .expect("warning expected");
    assert_eq!(warning.code, GuardrailCode::ExactFailureRepeat);
    assert_eq!(warning.count, 2);
    assert!(
        warning
            .render_suffix()
            .starts_with("\n\n[Tool loop warning:")
    );
}

#[test]
fn success_clears_exact_failure_slot() {
    let guard = LoopGuardrailHandle::from_config(&config(LoopGuardrailLevel::WarnOnly))
        .expect("handle expected");
    let args = json!({"file_path": "/tmp/x.rs"});
    assert!(
        guard
            .record_after_call(&read_tool(), &args, true, false, None)
            .is_none()
    );
    // Success resets the consecutive-failure count for this signature.
    let _ = guard.record_after_call(&read_tool(), &args, false, false, None);
    assert!(
        guard
            .record_after_call(&read_tool(), &args, true, false, None)
            .is_none(),
        "post-success failure count restarts from 1"
    );
}

#[test]
fn same_tool_distinct_args_warns_at_threshold() {
    let guard = LoopGuardrailHandle::from_config(&config(LoopGuardrailLevel::WarnOnly))
        .expect("handle expected");
    for i in 0..2 {
        let args = json!({"command": format!("cmd-{i}")});
        assert!(
            guard
                .record_after_call(&bash_tool(), &args, true, false, None)
                .is_none()
        );
    }
    // 3rd distinct-args failure of the same tool: warn (same_tool_failure=3).
    let warning = guard
        .record_after_call(
            &bash_tool(),
            &json!({"command": "cmd-2"}),
            true,
            false,
            None,
        )
        .expect("warning expected");
    assert_eq!(warning.code, GuardrailCode::SameToolFailures);
    assert_eq!(warning.count, 3);
}

#[test]
fn no_progress_warns_only_for_idempotent_identical_results() {
    let guard = LoopGuardrailHandle::from_config(&config(LoopGuardrailLevel::WarnOnly))
        .expect("handle expected");
    let args = json!({"file_path": "/tmp/x.rs"});
    // First identical-result success = count 1 (below no_progress=2).
    assert!(
        guard
            .record_after_call(&read_tool(), &args, false, true, Some("same output"))
            .is_none()
    );
    let warning = guard
        .record_after_call(&read_tool(), &args, false, true, Some("same output"))
        .expect("warning expected");
    assert_eq!(warning.code, GuardrailCode::NoProgressRepeat);
    assert_eq!(warning.count, 2);

    // A different result resets the repeat count.
    assert!(
        guard
            .record_after_call(&read_tool(), &args, false, true, Some("changed output"))
            .is_none()
    );

    // Mutating (non-idempotent) tools are never no-progress-flagged.
    let mutating_args = json!({"command": "date"});
    for _ in 0..4 {
        assert!(
            guard
                .record_after_call(&bash_tool(), &mutating_args, false, false, Some("same"))
                .is_none()
        );
    }
}

#[test]
fn warn_only_never_blocks() {
    let guard = LoopGuardrailHandle::from_config(&config(LoopGuardrailLevel::WarnOnly))
        .expect("handle expected");
    let args = json!({"file_path": "/tmp/x.rs"});
    for _ in 0..10 {
        let _ = guard.record_after_call(&read_tool(), &args, true, false, None);
    }
    assert!(guard.check_block_before_call(&read_tool(), &args).is_none());
}

#[test]
fn enforce_blocks_exact_failure_at_hard_stop() {
    let guard = LoopGuardrailHandle::from_config(&config(LoopGuardrailLevel::Enforce))
        .expect("handle expected");
    let args = json!({"file_path": "/tmp/x.rs"});
    for _ in 0..4 {
        let _ = guard.record_after_call(&read_tool(), &args, true, false, None);
        assert!(
            guard.check_block_before_call(&read_tool(), &args).is_none(),
            "below hard_stop_after=5 the call still executes"
        );
    }
    let _ = guard.record_after_call(&read_tool(), &args, true, false, None);
    let block = guard
        .check_block_before_call(&read_tool(), &args)
        .expect("block expected at 5 failures");
    assert_eq!(block.code, GuardrailCode::ExactFailureRepeat);
    assert_eq!(block.count, 5);
    let synthetic: serde_json::Value =
        serde_json::from_str(&block.render_synthetic_result()).expect("valid JSON");
    assert_eq!(synthetic["guardrail"]["code"], "exact_failure_repeat");
    assert!(synthetic["error"].is_string());

    // Different args on the same tool are NOT blocked by the exact gate.
    assert!(
        guard
            .check_block_before_call(&read_tool(), &json!({"file_path": "/tmp/other.rs"}))
            .is_none()
    );
}

#[test]
fn enforce_halts_same_tool_at_hard_stop() {
    let guard = LoopGuardrailHandle::from_config(&config(LoopGuardrailLevel::Enforce))
        .expect("handle expected");
    for i in 0..8 {
        let args = json!({"command": format!("cmd-{i}")});
        let _ = guard.record_after_call(&bash_tool(), &args, true, false, None);
    }
    let block = guard
        .check_block_before_call(&bash_tool(), &json!({"command": "cmd-new"}))
        .expect("halt expected at 8 tool failures");
    assert_eq!(block.code, GuardrailCode::SameToolFailures);
    assert!(block.message.contains("I stopped retrying"));
}

#[test]
fn signature_is_key_order_insensitive() {
    let guard = LoopGuardrailHandle::from_config(&config(LoopGuardrailLevel::WarnOnly))
        .expect("handle expected");
    let a = json!({"file_path": "/tmp/x.rs", "limit": 10});
    let b = json!({"limit": 10, "file_path": "/tmp/x.rs"});
    assert!(
        guard
            .record_after_call(&read_tool(), &a, true, false, None)
            .is_none()
    );
    // Same semantic args (different key order) must hit the same slot.
    let warning = guard
        .record_after_call(&read_tool(), &b, true, false, None)
        .expect("warning expected");
    assert_eq!(warning.code, GuardrailCode::ExactFailureRepeat);
    assert_eq!(warning.count, 2);
}
