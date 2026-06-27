//! Tests for the scheduler DTOs in [`crate::call_plan`].
//!
//! Locks in the type-system guarantees:
//!
//! - `completion_seq` is stamped only by the executor (via
//!   `stamp_and_extract_effects`).
//! - `ToolCallOutcome` is patch-free once stamped — an `AppStatePatch`
//!   returned by the runner lands in [`ToolSideEffects`], not the
//!   history-facing outcome.
//! - `ToolCallErrorKind` correctly classifies which errors run
//!   PostToolUseFailure and which `ToolMessagePath` they belong to.

use coco_types::ToolId;
use coco_types::ToolName;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

#[test]
fn test_stamp_assigns_completion_seq_and_copies_fields() {
    let unstamped = UnstampedToolCallOutcome {
        tool_use_id: "tu-1".into(),
        tool_id: ToolId::Builtin(ToolName::Read),
        model_index: 2,
        ordered_messages: vec![],
        message_path: ToolMessagePath::Success,
        error_kind: None,
        permission_denial: None,
        prevent_continuation: None,
        structured_output: None,
        effects: ToolSideEffects::none(),
    };
    let (stamped, _effects) = unstamped.stamp_and_extract_effects(/*seq*/ 7);
    assert_eq!(stamped.tool_use_id(), "tu-1");
    assert_eq!(stamped.tool_id(), &ToolId::Builtin(ToolName::Read));
    assert_eq!(stamped.model_index(), 2);
    assert_eq!(stamped.completion_seq(), 7);
    assert_eq!(stamped.message_path(), ToolMessagePath::Success);
    assert!(stamped.error_kind().is_none());
    assert!(stamped.permission_denial().is_none());
    assert!(stamped.prevent_continuation().is_none());
    assert!(stamped.ordered_messages().is_empty());
}

#[test]
fn test_stamp_splits_app_state_patch_into_side_effects() {
    // An AppStatePatch (FnOnce) carried in the unstamped body MUST
    // land in ToolSideEffects at stamp time, not in the history-facing
    // outcome. The ToolCallOutcome struct exposes no accessor for
    // patches, so this is enforced at compile time; here we verify
    // that the patch is still callable from the effects side.
    let unstamped = UnstampedToolCallOutcome {
        tool_use_id: "tu-1".into(),
        tool_id: ToolId::Builtin(ToolName::Read),
        model_index: 0,
        ordered_messages: vec![],
        message_path: ToolMessagePath::Success,
        error_kind: None,
        permission_denial: None,
        prevent_continuation: None,
        structured_output: None,
        effects: ToolSideEffects {
            app_state_patch: Some(Box::new(|state| {
                state.has_exited_plan_mode = true;
            })),
            permission_updates: Vec::new(),
        },
    };
    let (stamped, effects) = unstamped.stamp_and_extract_effects(/*seq*/ 0);

    // Outcome is patch-free — the type doesn't even expose patches.
    assert!(stamped.ordered_messages().is_empty());

    // Effects carries the patch, and applying it mutates state.
    let patch = effects.app_state_patch.expect("patch must move to effects");
    let mut state = coco_types::ToolAppState::default();
    assert!(!state.has_exited_plan_mode);
    patch(&mut state);
    assert!(state.has_exited_plan_mode);
}

#[test]
fn test_into_parts_consumes_outcome() {
    let unstamped = UnstampedToolCallOutcome {
        tool_use_id: "tu-1".into(),
        tool_id: ToolId::Builtin(ToolName::Read),
        model_index: 1,
        ordered_messages: vec![],
        message_path: ToolMessagePath::Success,
        error_kind: None,
        permission_denial: None,
        prevent_continuation: Some("stop reason".into()),
        structured_output: None,
        effects: ToolSideEffects::none(),
    };
    let (stamped, _) = unstamped.stamp_and_extract_effects(3);
    let parts = stamped.into_parts();
    assert_eq!(parts.tool_use_id, "tu-1");
    assert_eq!(parts.model_index, 1);
    assert_eq!(parts.completion_seq, 3);
    assert_eq!(parts.prevent_continuation.as_deref(), Some("stop reason"));
}

#[test]
fn test_error_kind_runs_post_tool_use_failure_matches_ts_parity() {
    // Execution-stage → YES
    assert!(ToolCallErrorKind::ExecutionFailed.runs_post_tool_use_failure());
    assert!(ToolCallErrorKind::ExecutionCancelled.runs_post_tool_use_failure());
    assert!(ToolCallErrorKind::JoinFailed.runs_post_tool_use_failure());

    // Pre-execution / early-return → NO
    assert!(!ToolCallErrorKind::UnknownTool.runs_post_tool_use_failure());
    assert!(!ToolCallErrorKind::SchemaFailed.runs_post_tool_use_failure());
    assert!(!ToolCallErrorKind::ValidationFailed.runs_post_tool_use_failure());
    assert!(!ToolCallErrorKind::HookBlocked.runs_post_tool_use_failure());
    assert!(!ToolCallErrorKind::PermissionDenied.runs_post_tool_use_failure());
    assert!(!ToolCallErrorKind::PermissionBridgeFailed.runs_post_tool_use_failure());
    assert!(!ToolCallErrorKind::PreExecutionCancelled.runs_post_tool_use_failure());
    assert!(!ToolCallErrorKind::StreamingDiscarded.runs_post_tool_use_failure());
}

#[test]
fn test_error_kind_message_path_aligns_with_post_hook_gate() {
    // Execution-stage errors travel the Failure template (post-hook
    // runs as PostToolUseFailure); pre-execution errors travel the
    // EarlyReturn template (no post-hook). The two predicates must
    // agree — message_path is derived from runs_post_tool_use_failure.
    for kind in [
        ToolCallErrorKind::ExecutionFailed,
        ToolCallErrorKind::ExecutionCancelled,
        ToolCallErrorKind::JoinFailed,
    ] {
        assert_eq!(kind.message_path(), ToolMessagePath::Failure, "{kind:?}");
    }
    for kind in [
        ToolCallErrorKind::UnknownTool,
        ToolCallErrorKind::SchemaFailed,
        ToolCallErrorKind::ValidationFailed,
        ToolCallErrorKind::HookBlocked,
        ToolCallErrorKind::PermissionDenied,
        ToolCallErrorKind::PermissionBridgeFailed,
        ToolCallErrorKind::PreExecutionCancelled,
        ToolCallErrorKind::StreamingDiscarded,
    ] {
        assert_eq!(
            kind.message_path(),
            ToolMessagePath::EarlyReturn,
            "{kind:?}"
        );
    }
}

#[test]
fn test_early_outcome_plan_can_wrap_unstamped_outcome() {
    // EarlyOutcome is the plan variant for pre-execution failures.
    // The outcome body is unstamped so the executor can stamp it when
    // it reaches the barrier block — not before.
    let plan = ToolCallPlan::EarlyOutcome(UnstampedToolCallOutcome {
        tool_use_id: "tu-1".into(),
        tool_id: ToolId::Custom("unknown-tool".into()),
        model_index: 0,
        ordered_messages: vec![],
        message_path: ToolMessagePath::EarlyReturn,
        error_kind: Some(ToolCallErrorKind::UnknownTool),
        permission_denial: None,
        prevent_continuation: None,
        structured_output: None,
        effects: ToolSideEffects::none(),
    });
    match plan {
        ToolCallPlan::EarlyOutcome(o) => {
            assert_eq!(o.error_kind, Some(ToolCallErrorKind::UnknownTool));
            assert_eq!(o.message_path, ToolMessagePath::EarlyReturn);
        }
        ToolCallPlan::Runnable(_) => panic!("expected EarlyOutcome"),
    }
}

#[test]
fn test_runnable_plan_carries_prepared_call() {
    // Sanity: `Runnable(PreparedToolCall)` holds all the fields
    // `run_one` needs before the per-tool semantic lifecycle starts.
    struct DummyTool;
    #[async_trait::async_trait]
    impl crate::traits::Tool for DummyTool {
        fn runtime_validation_schema(&self) -> &crate::schema::ToolInputSchema {
            crate::schema::test_runtime_schema()
        }
        // Migration scaffold: assoc types pinned to `Value`.
        type Input = serde_json::Value;
        type Output = serde_json::Value;

        fn id(&self) -> ToolId {
            ToolId::Custom("dummy".into())
        }
        fn name(&self) -> &str {
            "dummy"
        }
        fn description(
            &self,
            _: &serde_json::Value,
            _: &crate::traits::DescriptionOptions,
        ) -> String {
            "dummy".into()
        }
        async fn prompt(&self, _options: &crate::traits::PromptOptions) -> String {
            "test tool".into()
        }
        async fn execute(
            &self,
            _input: serde_json::Value,
            _ctx: &crate::context::ToolUseContext,
        ) -> Result<coco_messages::ToolResult<serde_json::Value>, crate::error::ToolError> {
            Ok(coco_messages::ToolResult {
                data: json!({}),
                new_messages: vec![],
                app_state_patch: None,
                permission_updates: Vec::new(),
                display_data: None,
            })
        }
    }
    let prepared = PreparedToolCall {
        tool_use_id: "tu-1".into(),
        tool_id: ToolId::Custom("dummy".into()),
        tool: std::sync::Arc::new(DummyTool),
        parsed_input: crate::ValidatedInput::validate(&DummyTool, json!({"x": 1}))
            .expect("test input must validate"),
        is_concurrency_safe: false,
        model_index: 5,
        permission_resolution_detail: None,
        approval_feedback: None,
    };
    let plan = ToolCallPlan::Runnable(prepared);
    match plan {
        ToolCallPlan::Runnable(p) => {
            assert_eq!(p.tool_use_id, "tu-1");
            assert_eq!(p.model_index, 5);
            assert_eq!(p.parsed_input.as_value(), &json!({"x": 1}));
        }
        ToolCallPlan::EarlyOutcome(_) => panic!("expected Runnable"),
    }
}
