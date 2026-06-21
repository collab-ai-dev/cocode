use std::sync::Arc;

use coco_llm_types::ToolCallPart;
use coco_tool_runtime::CanUseToolCallContext;
use coco_tool_runtime::CanUseToolDecision;
use coco_tool_runtime::CanUseToolHandle;
use coco_tool_runtime::DecisionReason;
use coco_tool_runtime::ToolUseContext;
use coco_types::PermissionBehavior;
use coco_types::PermissionDecision;
use coco_types::PermissionDecisionReason;
use serde_json::json;

use super::*;

use coco_inference::AISdkError;
use coco_inference::LanguageModel;
use coco_inference::LanguageModelCallOptions;
use coco_inference::LanguageModelGenerateResult;
use coco_inference::LanguageModelStreamResult;
use coco_llm_types::AssistantContentPart;
use coco_llm_types::FinishReason;
use coco_llm_types::StopReason;
use coco_llm_types::TextPart;
use coco_llm_types::Usage;
use coco_messages::ToolResult;
use coco_permissions::AutoModeState;
use coco_permissions::DenialTracker;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::PromptOptions;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolInputSchema;
use coco_types::PermissionMode;
use coco_types::ToolCheckResult;
use tokio::sync::Mutex;

#[derive(Debug)]
struct AlwaysDenyHandle;

#[async_trait::async_trait]
impl CanUseToolHandle for AlwaysDenyHandle {
    async fn check(
        &self,
        _tool_name: &str,
        _input: &serde_json::Value,
        _ctx: &CanUseToolCallContext,
    ) -> CanUseToolDecision {
        CanUseToolDecision::Deny {
            message: "fork blocked tool".into(),
            decision_reason: DecisionReason::Other {
                reason: "fork_policy".into(),
            },
        }
    }
}

#[derive(Debug)]
struct AlwaysAllowRewriteHandle;

#[async_trait::async_trait]
impl CanUseToolHandle for AlwaysAllowRewriteHandle {
    async fn check(
        &self,
        _tool_name: &str,
        _input: &serde_json::Value,
        _ctx: &CanUseToolCallContext,
    ) -> CanUseToolDecision {
        CanUseToolDecision::Allow {
            updated_input: Some(json!({"file_path": "/overlay/foo.txt"})),
            decision_reason: DecisionReason::Other {
                reason: "rewrite".into(),
            },
        }
    }
}

fn tool_call() -> ToolCallPart {
    ToolCallPart {
        tool_call_id: "call-1".into(),
        tool_name: "Read".into(),
        input: json!({"file_path": "/main/foo.txt"}),
        provider_executed: None,
        invalid: false,
        invalid_reason: None,
        provider_metadata: None,
    }
}

#[tokio::test]
async fn test_can_use_tool_deny_becomes_permission_deny_in_preparer() {
    let mut ctx = ToolUseContext::test_default();
    ctx.can_use_tool = Some(Arc::new(AlwaysDenyHandle));

    let resolution = resolve_can_use_tool_decision(
        &tool_call(),
        &json!({"file_path": "/main/foo.txt"}),
        &ctx,
        None,
    )
    .await
    .expect("canUseTool decision should run");

    match resolution {
        CanUseToolResolution::Decision(PermissionDecision::Deny { message, reason }) => {
            assert_eq!(message, "fork blocked tool");
            match reason {
                PermissionDecisionReason::AsyncAgent { reason } => {
                    assert_eq!(reason, "fork_policy");
                }
                other => panic!("expected AsyncAgent reason, got {other:?}"),
            }
        }
        CanUseToolResolution::Decision(other) => {
            panic!("expected Deny decision, got {other:?}");
        }
        CanUseToolResolution::Ask => panic!("expected concrete Deny decision"),
    }
}

#[tokio::test]
async fn test_can_use_tool_allow_rewrite_becomes_permission_allow() {
    let mut ctx = ToolUseContext::test_default();
    ctx.can_use_tool = Some(Arc::new(AlwaysAllowRewriteHandle));

    let resolution = resolve_can_use_tool_decision(
        &tool_call(),
        &json!({"file_path": "/main/foo.txt"}),
        &ctx,
        None,
    )
    .await
    .expect("canUseTool decision should run");

    match resolution {
        CanUseToolResolution::Decision(PermissionDecision::Allow {
            updated_input,
            feedback,
        }) => {
            assert_eq!(
                updated_input,
                Some(json!({"file_path": "/overlay/foo.txt"}))
            );
            assert_eq!(feedback.as_deref(), Some("rewrite"));
        }
        CanUseToolResolution::Decision(other) => {
            panic!("expected Allow decision, got {other:?}");
        }
        CanUseToolResolution::Ask => panic!("expected concrete Allow decision"),
    }
}

#[tokio::test]
async fn test_hook_allow_bypasses_can_use_tool_unless_required() {
    let mut ctx = ToolUseContext::test_default();
    ctx.can_use_tool = Some(Arc::new(AlwaysDenyHandle));

    let skipped = resolve_can_use_tool_decision(
        &tool_call(),
        &json!({"file_path": "/main/foo.txt"}),
        &ctx,
        Some(PermissionBehavior::Allow),
    )
    .await;
    assert!(
        skipped.is_none(),
        "normal hook allow should preserve existing auto-approve semantics"
    );

    ctx.require_can_use_tool = true;
    let enforced = resolve_can_use_tool_decision(
        &tool_call(),
        &json!({"file_path": "/main/foo.txt"}),
        &ctx,
        Some(PermissionBehavior::Allow),
    )
    .await;
    assert!(
        matches!(
            enforced,
            Some(CanUseToolResolution::Decision(
                PermissionDecision::Deny { .. }
            ))
        ),
        "require_can_use_tool must force the fork policy to run"
    );
}

// ── Auto-mode overlay must not swallow interaction tools (AskUserQuestion) ──
//
// Regression for: in Auto mode the auto-mode classifier overlay treated
// AskUserQuestion's `Ask` as a security prompt, hit the safe-tool allowlist
// (`is_safe_tool` == true), and rewrote it to `Allow`. The permission bridge
// then never fired and the interactive question overlay was silently dropped.
// The fix gates the overlay on `!tool.requires_user_interaction()`.

/// Minimal `LanguageModel` — only needed to construct a `ModelRuntimeRegistry`.
/// The classifier never runs in these tests (safe tools short-circuit before
/// the model is queried), so `do_generate` is never actually called.
struct StubModel;

#[async_trait::async_trait]
impl LanguageModel for StubModel {
    fn provider(&self) -> &str {
        "stub"
    }
    fn model_id(&self) -> &str {
        "stub"
    }
    async fn do_generate(
        &self,
        _options: &LanguageModelCallOptions,
        _abort_signal: Option<CancellationToken>,
    ) -> Result<LanguageModelGenerateResult, AISdkError> {
        Ok(LanguageModelGenerateResult {
            content: vec![AssistantContentPart::Text(TextPart {
                text: String::new(),
                provider_metadata: None,
            })],
            usage: Usage::new(0, 0),
            finish_reason: FinishReason::new(StopReason::EndTurn),
            warnings: vec![],
            provider_metadata: None,
            request: None,
            response: None,
        })
    }
    async fn do_stream(
        &self,
        options: &LanguageModelCallOptions,
        _abort_signal: Option<CancellationToken>,
    ) -> Result<LanguageModelStreamResult, AISdkError> {
        let result = self.do_generate(options, None).await?;
        Ok(coco_inference::synthetic_stream_from_content(
            result.content,
            result.usage,
            result.finish_reason,
        ))
    }
}

/// A tool that always returns `Ask` from `check_permissions` and borrows the
/// `AskUserQuestion` id so the auto-mode safe-tool allowlist recognizes it —
/// letting us exercise the overlay path without a live classifier. The
/// `requires_interaction` flag is the only thing that varies between the two
/// assertions in `auto_mode_overlay_gates_on_requires_user_interaction`.
struct AskingMockTool {
    requires_interaction: bool,
}

fn mock_schema() -> &'static ToolInputSchema {
    static S: std::sync::OnceLock<ToolInputSchema> = std::sync::OnceLock::new();
    S.get_or_init(|| {
        ToolInputSchema::from_value(json!({"type": "object"})).expect("valid mock schema")
    })
}

#[async_trait::async_trait]
impl Tool for AskingMockTool {
    type Input = Value;
    type Output = Value;

    fn runtime_validation_schema(&self) -> &ToolInputSchema {
        mock_schema()
    }

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::AskUserQuestion)
    }

    fn name(&self) -> &str {
        ToolName::AskUserQuestion.as_str()
    }

    fn description(&self, _input: &Value, _options: &DescriptionOptions) -> String {
        "mock interactive tool".into()
    }

    async fn prompt(&self, _options: &PromptOptions) -> String {
        "mock".into()
    }

    fn requires_user_interaction(&self) -> bool {
        self.requires_interaction
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> ToolCheckResult {
        ToolCheckResult::Ask {
            message: "answer?".into(),
            suggestions: Vec::new(),
            choices: None,
            detail: None,
        }
    }

    async fn execute(
        &self,
        input: Value,
        _ctx: &ToolUseContext,
    ) -> Result<ToolResult<Value>, ToolError> {
        Ok(ToolResult {
            data: input,
            new_messages: vec![],
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}

fn ask_tool_call() -> ToolCallPart {
    ToolCallPart {
        tool_call_id: "call-ask".into(),
        tool_name: ToolName::AskUserQuestion.as_str().into(),
        input: json!({ "questions": [] }),
        provider_executed: None,
        invalid: false,
        invalid_reason: None,
        provider_metadata: None,
    }
}

/// Drive `resolve_permission_decision` for `tool` with the auto-mode overlay
/// fully armed (state active, tracker present). `model_runtimes`/`tools` are
/// real but inert — the safe-tool fast path returns before the classifier.
async fn decide_in_auto_mode(tool: Arc<dyn DynTool>) -> PermissionDecision {
    let mut ctx = ToolUseContext::test_default();
    ctx.permission_context.mode = PermissionMode::Auto;

    let auto = Arc::new(AutoModeState::new());
    auto.set_active(true);
    let tracker = Arc::new(Mutex::new(DenialTracker::new()));
    let model_runtimes = crate::test_support::model_runtime_registry(Arc::new(StubModel));
    let rules = AutoModeRules::default();
    let tools = ToolRegistry::new();
    let history: Vec<Message> = Vec::new();
    let tc = ask_tool_call();

    resolve_permission_decision(
        &tc,
        &tool,
        &tc.input,
        &ctx,
        history.as_slice(),
        (None, None),
        Some(&auto),
        Some(&tracker),
        &model_runtimes,
        &rules,
        &tools,
    )
    .await
}

#[tokio::test]
async fn auto_mode_preserves_ask_for_ask_user_question() {
    let tool: Arc<dyn DynTool> = Arc::new(coco_tools::AskUserQuestionTool);
    let decision = decide_in_auto_mode(tool).await;
    assert!(
        matches!(decision, PermissionDecision::Ask { .. }),
        "Auto mode must NOT auto-allow AskUserQuestion via the safe-tool \
         allowlist — the interactive overlay would be silently dropped. \
         got {decision:?}"
    );
}

#[tokio::test]
async fn auto_mode_overlay_gates_on_requires_user_interaction() {
    // Identical safe-tool id + identical `Ask` from check_permissions; the ONLY
    // difference is `requires_user_interaction()`. The interactive variant keeps
    // `Ask` (overlay skipped → bridge fires); the non-interactive variant is
    // auto-allowed by the safe-tool allowlist (overlay runs). Pins the fix to
    // the interaction flag and proves it didn't disable the overlay wholesale.
    let interactive = decide_in_auto_mode(Arc::new(AskingMockTool {
        requires_interaction: true,
    }))
    .await;
    assert!(
        matches!(interactive, PermissionDecision::Ask { .. }),
        "interactive tool's Ask must survive the auto-mode overlay, got {interactive:?}"
    );

    let non_interactive = decide_in_auto_mode(Arc::new(AskingMockTool {
        requires_interaction: false,
    }))
    .await;
    assert!(
        matches!(non_interactive, PermissionDecision::Allow { .. }),
        "non-interactive safe tool must still be auto-allowed by the overlay, \
         got {non_interactive:?}"
    );
}

#[tokio::test]
async fn bypass_mode_preserves_ask_for_ask_user_question() {
    // Bypass mode is structurally immune already: the evaluator returns the
    // tool's step-1c `Ask` (before `mode_fallthrough`'s BypassPermissions →
    // Allow), and the auto-mode overlay never runs because auto mode is not
    // active. This pins that the single Auto-mode fix did not need a bypass
    // counterpart and that bypass still surfaces the interactive prompt.
    let tool: Arc<dyn DynTool> = Arc::new(coco_tools::AskUserQuestionTool);
    let ctx = ToolUseContext::test_default(); // mode = BypassPermissions by default
    let model_runtimes = crate::test_support::model_runtime_registry(Arc::new(StubModel));
    let rules = AutoModeRules::default();
    let tools = ToolRegistry::new();
    let history: Vec<Message> = Vec::new();
    let tc = ask_tool_call();

    let decision = resolve_permission_decision(
        &tc,
        &tool,
        &tc.input,
        &ctx,
        history.as_slice(),
        (None, None),
        None, // auto mode not active in bypass
        None,
        &model_runtimes,
        &rules,
        &tools,
    )
    .await;

    assert!(
        matches!(decision, PermissionDecision::Ask { .. }),
        "Bypass mode must surface AskUserQuestion's Ask so the interactive \
         overlay still fires. got {decision:?}"
    );
}

// ── auto-mode classifier gate: per-call context mode, not a shared flag ──

#[test]
fn should_auto_classify_reads_per_call_mode_not_shared_flag() {
    use coco_permissions::AutoModeState;
    use coco_types::PermissionMode;
    use std::sync::Arc;

    // Regression for the subagent clobber bug: a shared `AutoModeState` flipped
    // INACTIVE by a concurrent fork/sub-engine build must NOT suppress the
    // classifier for a call whose per-call context mode is `Auto`.
    let inactive = Arc::new(AutoModeState::new()); // is_active() == false
    assert!(
        should_auto_classify(PermissionMode::Auto, Some(&inactive)),
        "Auto mode must classify regardless of the shared flag"
    );
    // Even with NO shared state at all, Auto classifies.
    assert!(should_auto_classify(PermissionMode::Auto, None));

    // Non-auto modes never classify.
    for mode in [
        PermissionMode::Default,
        PermissionMode::AcceptEdits,
        PermissionMode::BypassPermissions,
        PermissionMode::DontAsk,
    ] {
        assert!(!should_auto_classify(mode, Some(&inactive)), "{mode:?}");
    }
}

#[test]
fn should_auto_classify_plan_bridges_only_when_flag_active() {
    use coco_permissions::AutoModeState;
    use coco_types::PermissionMode;
    use std::sync::Arc;

    // TS parity: `mode === 'plan' && isAutoModeActive()`. Plan bridges to the
    // classifier ONLY when the narrowly-scoped auto flag is set.
    let inactive = Arc::new(AutoModeState::new());
    assert!(!should_auto_classify(PermissionMode::Plan, Some(&inactive)));
    assert!(!should_auto_classify(PermissionMode::Plan, None));

    let active = Arc::new(AutoModeState::new());
    active.set_active(true);
    assert!(should_auto_classify(PermissionMode::Plan, Some(&active)));
}
