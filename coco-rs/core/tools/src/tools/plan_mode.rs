//! EnterPlanMode + ExitPlanMode tools.
//!
//! Plan mode is a permission mode in which the model researches and
//! designs an implementation approach but MUST NOT modify the system.
//! Entry + exit are mediated by these two tools; the per-turn reminder
//! text and the permission-gate defaults live in `core/context` and
//! `core/permissions` respectively.

use coco_messages::ToolResult;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::PlanApprovalMessage;
use coco_tool_runtime::PlanApprovalRequest;
use coco_tool_runtime::PromptOptions;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolResultContentPart;
use coco_tool_runtime::ToolUseContext;
use coco_tool_runtime::ValidationResult;
use coco_types::ExitPlanChoice;
use coco_types::ExitPlanModeAllowedPrompt as PermissionExitPlanAllowedPrompt;
use coco_types::ExitPlanModeOutcome;
use coco_types::ExitPlanModeResult;
use coco_types::PendingPlanVerificationState;
use coco_types::PermissionMode;
use coco_types::PermissionRequestDetail;
use coco_types::ToolDisplayData;
use coco_types::ToolId;
use coco_types::ToolName;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

/// Typed result payload for [`ExitPlanModeTool`]. Producers: `execute`
/// (creates it) serializes to `Value` at the Tool-trait boundary.
/// Consumers: `build_instructions` deserializes back here. Per
/// CLAUDE.md "Typed Structs over JSON Values": both sides live in the
/// same crate, so stringly-typed `Value.get("key")` access is a code
/// smell.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct ExitPlanModeOutput {
    /// Whether this exit carries an implementation plan.
    outcome: ExitPlanModeOutcome,
    /// The plan text — either the CCR-edited version from `input.plan`
    /// or the on-disk plan file contents.
    #[serde(skip_serializing_if = "Option::is_none")]
    plan: Option<String>,
    /// True when called from a subagent context. Shortens the approval
    /// text the model receives.
    is_agent: bool,
    /// Absolute path to the session's plan file.
    #[serde(skip_serializing_if = "Option::is_none")]
    file_path: Option<String>,
    /// Hint that the TeamCreate tool is available — surfaced as a hint
    /// in the approval message to encourage swarm-based implementation.
    #[serde(skip_serializing_if = "Option::is_none")]
    has_task_tool: Option<bool>,
    /// True when the CCR UI edited the plan via `input.plan` before exit.
    #[serde(skip_serializing_if = "Option::is_none")]
    plan_was_edited: Option<bool>,
    /// Teammate-awaiting-approval branch: the teammate's tool call
    /// submitted the plan to the lead and must stay in plan mode until
    /// the response arrives.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    awaiting_leader_approval: bool,
    /// Correlation ID for the pending approval (only set when
    /// `awaiting_leader_approval` is true).
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<String>,
}

const APPROVAL_FEEDBACK_CONTEXT_CHAR_LIMIT: usize = 4_000;
const APPROVAL_FEEDBACK_TRUNCATION_MARKER: &str =
    "\n\n[Approval feedback truncated. The approved plan is saved at the plan file path above.]";

/// Build the cross-turn `AppStatePatch` that flips state into plan
/// mode. Captures the current mode (so `ExitPlanMode` knows where to
/// restore) and stamps the entry timestamp so `ExitPlanMode` can
/// optionally compare against the plan-file mtime. The same shape is
/// applied at two entry sites in coco-rs: the tool's own `execute`
/// and the `/plan <description>` slash command, which flips state
/// directly without re-prompting the user.
pub fn build_enter_plan_mode_patch(current_mode: PermissionMode) -> coco_types::AppStatePatch {
    Box::new(move |state| {
        // Plan entry only — the Auto-entry stash branch is inert here, so an
        // empty allow-rules map suffices.
        coco_permissions::apply_permission_mode_transition_to_app_state(
            state,
            current_mode,
            PermissionMode::Plan,
            &coco_types::PermissionRulesBySource::new(),
        );
        state.pending_plan_mode_exit_outcome = None;
    })
}

// ── EnterPlanModeTool ──

pub struct EnterPlanModeTool;

/// Full prompt text for the model.
///
/// The Ant arm and the `USER_TYPE === 'ant'` dispatcher are intentionally NOT
/// ported (root `CLAUDE.md` "no Ant gates"). Interpolates
/// `ToolName::AskUserQuestion.as_str()` so a future tool rename
/// propagates without a doc edit. Names like `EnterPlanMode` /
/// `ExitPlanMode` stay as literals — the wire names are stable.
fn enter_plan_mode_prompt(is_plan_interview_phase: bool) -> String {
    let aq = coco_types::ToolName::AskUserQuestion.as_str();
    // Empty string when the iterative interview workflow is active
    // (the model gets the detailed loop via the plan-mode attachment instead).
    let what_happens = if is_plan_interview_phase {
        String::new()
    } else {
        format!(
            "## What Happens in Plan Mode

In plan mode, you'll:
1. Thoroughly explore the codebase using Glob, Grep, and Read tools
2. Understand existing patterns and architecture
3. Design an implementation approach
4. Present your plan to the user for approval
5. Use {aq} if you need to clarify approaches
6. Exit plan mode with ExitPlanMode when ready to implement

"
        )
    };
    format!(
        "Use this tool proactively when you're about to start a non-trivial implementation task. Getting user sign-off on your approach before writing code prevents wasted effort and ensures alignment. This tool transitions you into plan mode where you can explore the codebase and design an implementation approach for user approval.

## When to Use This Tool

**Prefer using EnterPlanMode** for implementation tasks unless they're simple. Use it when ANY of these conditions apply:

1. **New Feature Implementation**: Adding meaningful new functionality
   - Example: \"Add a logout button\" - where should it go? What should happen on click?
   - Example: \"Add form validation\" - what rules? What error messages?

2. **Multiple Valid Approaches**: The task can be solved in several different ways
   - Example: \"Add caching to the API\" - could use Redis, in-memory, file-based, etc.
   - Example: \"Improve performance\" - many optimization strategies possible

3. **Code Modifications**: Changes that affect existing behavior or structure
   - Example: \"Update the login flow\" - what exactly should change?
   - Example: \"Refactor this component\" - what's the target architecture?

4. **Architectural Decisions**: The task requires choosing between patterns or technologies
   - Example: \"Add real-time updates\" - WebSockets vs SSE vs polling
   - Example: \"Implement state management\" - Redux vs Context vs custom solution

5. **Multi-File Changes**: The task will likely touch more than 2-3 files
   - Example: \"Refactor the authentication system\"
   - Example: \"Add a new API endpoint with tests\"

6. **Unclear Requirements**: You need to explore before understanding the full scope
   - Example: \"Make the app faster\" - need to profile and identify bottlenecks
   - Example: \"Fix the bug in checkout\" - need to investigate root cause

7. **User Preferences Matter**: The implementation could reasonably go multiple ways
   - If you would use {aq} to clarify the approach, use EnterPlanMode instead
   - Plan mode lets you explore first, then present options with context

## When NOT to Use This Tool

Only skip EnterPlanMode for simple tasks:
- Single-line or few-line fixes (typos, obvious bugs, small tweaks)
- Adding a single function with clear requirements
- Tasks where the user has given very specific, detailed instructions
- Pure research/exploration tasks (use the Agent tool with explore agent instead)

{what_happens}## Examples

### GOOD - Use EnterPlanMode:
User: \"Add user authentication to the app\"
- Requires architectural decisions (session vs JWT, where to store tokens, middleware structure)

User: \"Optimize the database queries\"
- Multiple approaches possible, need to profile first, significant impact

User: \"Implement dark mode\"
- Architectural decision on theme system, affects many components

User: \"Add a delete button to the user profile\"
- Seems simple but involves: where to place it, confirmation dialog, API call, error handling, state updates

User: \"Update the error handling in the API\"
- Affects multiple files, user should approve the approach

### BAD - Don't use EnterPlanMode:
User: \"Fix the typo in the README\"
- Straightforward, no planning needed

User: \"Add a console.log to debug this function\"
- Simple, obvious implementation

User: \"What files handle routing?\"
- Research task, not implementation planning

## Important Notes

- This tool REQUIRES user approval - they must consent to entering plan mode
- If unsure whether to use it, err on the side of planning - it's better to get alignment upfront than to redo work
- Users appreciate being consulted before significant changes are made to their codebase
"
    )
}

/// Typed input for [`EnterPlanModeTool`] — no parameters (empty input schema).
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct EnterPlanModeInput {}

#[async_trait::async_trait]
impl Tool for EnterPlanModeTool {
    type Input = EnterPlanModeInput;
    coco_tool_runtime::impl_runtime_schema!(EnterPlanModeInput);
    /// Output is `Value` — the renderer reads `message` +
    /// `isInterviewPhase` positionally. Could be a typed
    /// `{message, isInterviewPhase}` struct in a follow-up; current
    /// shape stays compatible with the existing test fixtures.
    type Output = Value;

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::EnterPlanMode)
    }
    fn name(&self) -> &str {
        ToolName::EnterPlanMode.as_str()
    }
    fn description(&self, _input: &EnterPlanModeInput, _options: &DescriptionOptions) -> String {
        "Requests permission to enter plan mode for complex tasks requiring \
         exploration and design"
            .into()
    }
    async fn prompt(&self, options: &PromptOptions) -> String {
        enter_plan_mode_prompt(options.is_plan_interview_phase)
    }
    fn user_facing_name(&self) -> &str {
        ""
    }
    fn is_read_only(&self, _input: &EnterPlanModeInput) -> bool {
        true
    }
    fn is_always_read_only(&self) -> bool {
        true
    }
    fn is_concurrency_safe(&self, _input: &EnterPlanModeInput) -> bool {
        true
    }
    // Not deferred: `should_defer` stays the trait default `false` so the
    // model can call `EnterPlanMode` directly, with no `ToolSearch` round-trip
    // (same rationale as the Task/Todo tools). `search_hint` is kept anyway —
    // it's inert while non-deferred, but ready if `should_defer` is flipped on.
    fn search_hint(&self) -> Option<&str> {
        Some("switch to plan mode to design an approach before coding")
    }
    /// Gated by the `plan_mode` feature (default on). When a user opts out
    /// (`features.plan_mode = false`), the whole plan-mode subsystem is
    /// hidden — the model never sees this tool.
    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.features.enabled(coco_types::Feature::PlanMode)
    }

    async fn execute(
        &self,
        _input: EnterPlanModeInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<Value>, ToolError> {
        if ctx.agent_id.is_some() {
            return Err(ToolError::InvalidInput {
                message: "EnterPlanMode tool cannot be used in agent contexts".into(),
                error_code: None,
            });
        }

        // Read live state (short-lived read lock) to decide what
        // the patch will do. `prev` in the conceptual closure is
        // our `current` snapshot.
        let current_mode = match ctx.app_state.as_ref() {
            Some(h) => h
                .read()
                .await
                .permissions
                .mode
                .unwrap_or(ctx.permission_context.mode),
            None => ctx.permission_context.mode,
        };

        // Queue the mutation. Executor applies it post-execute under a
        // write lock (`AppStateReadHandle` has no write surface). The
        // patch builder is shared with `dispatch_plan` so a typed `/plan
        // <description>` and a tool-driven entry land identical
        // app-state shape.
        let patch = build_enter_plan_mode_patch(current_mode);

        Ok(
            ToolResult::data(serde_json::json!({
                "message": "Entered plan mode. You should now focus on exploring the codebase and designing an implementation approach.",
                "isInterviewPhase": ctx.is_plan_interview_phase,
            }))
            .with_patch(patch),
        )
    }

    fn render_for_model(&self, data: &Value) -> Vec<ToolResultContentPart> {
        let message = data
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let is_interview = data
            .get("isInterviewPhase")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        vec![ToolResultContentPart::Text {
            text: Self::build_instructions(message, is_interview),
            provider_options: None,
        }]
    }
}

impl EnterPlanModeTool {
    /// Build the instructions text returned to the model as tool_result content.
    /// Two variants gated on the interview-phase flag — the terse "DO NOT write"
    /// version when the iterative interview workflow will deliver detailed steps
    /// via the plan-mode attachment, otherwise the 6-step fallback that doubles
    /// as the workflow primer.
    pub fn build_instructions(confirmation: &str, is_plan_interview_phase: bool) -> String {
        if is_plan_interview_phase {
            format!(
                "{confirmation}

DO NOT write or edit any files except the plan file. Detailed workflow instructions will follow."
            )
        } else {
            format!(
                "{confirmation}

In plan mode, you should:
1. Thoroughly explore the codebase to understand existing patterns
2. Identify similar features and architectural approaches
3. Consider multiple approaches and their trade-offs
4. Use AskUserQuestion if you need to clarify the approach
5. Design a concrete implementation strategy
6. When ready, use ExitPlanMode to present your plan for approval

Remember: DO NOT write or edit any files yet. This is a read-only exploration and planning phase."
            )
        }
    }
}

// ── ExitPlanModeTool ──

pub struct ExitPlanModeTool;

/// Full prompt text for the model.
///
/// Interpolates `ToolName::AskUserQuestion.as_str()` so a future rename of
/// the AskUserQuestion tool propagates without a doc edit.
fn exit_plan_mode_prompt() -> String {
    let aq = coco_types::ToolName::AskUserQuestion.as_str();
    format!(
        "Use this tool when you are in plan mode and have finished writing your plan to the plan file and are ready for user approval.\n\
         \n\
         ## How This Tool Works\n\
         - You should have already written your plan to the plan file specified in the plan mode system message\n\
         - This tool does NOT take the plan content as a parameter - it will read the plan from the file you wrote\n\
         - This tool simply signals that you're done planning and ready for the user to review and approve, or that no implementation plan is needed\n\
         - The user will see the contents of your plan file when they review it\n\
         \n\
         ## When to Use This Tool\n\
         IMPORTANT: Only use this tool when the task requires planning the implementation steps of a task that requires writing code. For research tasks where you're gathering information, searching files, reading files or in general trying to understand the codebase - do NOT use this tool.\n\
         \n\
         ## Before Using This Tool\n\
         Ensure your plan is complete and unambiguous:\n\
         - If you have unresolved questions about requirements or approach, use {aq} first (in earlier phases)\n\
         - Once your plan is finalized, use THIS tool to request approval\n\
         \n\
         **Important:** Do NOT use {aq} to ask \"Is this plan okay?\" or \"Should I proceed?\" - that's exactly what THIS tool does. ExitPlanMode inherently requests user approval of your plan.\n\
         "
    )
}

/// The tool an `allowedPrompts` entry pre-approves (only `Bash` supported).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema)]
pub enum AllowedPromptTool {
    #[default]
    Bash,
}

impl AllowedPromptTool {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Bash => "Bash",
        }
    }
}

/// Single entry in the `allowedPrompts` array — pre-approved tool / prompt
/// pair that the model is signaling it intends to use when plan is approved.
/// Both fields are required, so the derived schema carries
/// `required: [tool, prompt]` and the `tool` enum — no hand-patching needed.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ExitPlanAllowedPrompt {
    /// The tool this prompt applies to (only `Bash` is supported).
    pub tool: AllowedPromptTool,
    /// The exact command/prompt to pre-approve.
    pub prompt: String,
}

/// Typed input for [`ExitPlanModeTool`].
///
/// The only accepted field is `allowedPrompts`. Plan content for display is
/// carried in [`PermissionRequestDetail`], and approval choices/edits are
/// carried in `ToolUseContext.permission_resolution_detail`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ExitPlanModeInput {
    /// Prompt-based permissions needed to implement the plan.
    #[serde(default, rename = "allowedPrompts")]
    pub allowed_prompts: Option<Vec<ExitPlanAllowedPrompt>>,
}

fn build_exit_plan_mode_no_plan_choices() -> Vec<coco_types::PermissionAskChoice> {
    vec![
        coco_types::PermissionAskChoice {
            value: ExitPlanChoice::KeepDefault.as_str().into(),
            label: "Yes, exit plan mode".into(),
            description: None,
        },
        coco_types::PermissionAskChoice {
            value: ExitPlanChoice::No.as_str().into(),
            label: "No, keep planning".into(),
            description: None,
        },
    ]
}

fn build_exit_plan_mode_choices_for_state(
    state: &coco_context::ExitPlanModeDerivedState,
    ctx: &ToolUseContext,
) -> Vec<coco_types::PermissionAskChoice> {
    if state.outcome == ExitPlanModeOutcome::NoImplementationPlan {
        return build_exit_plan_mode_no_plan_choices();
    }
    let mut choices = Vec::new();
    if ctx.plan_mode_settings.show_clear_context_on_exit {
        if ctx.permission_context.bypass_available {
            choices.push(coco_types::PermissionAskChoice {
                value: ExitPlanChoice::ClearBypassPermissions.as_str().into(),
                label: "Yes, clear context and bypass permissions".into(),
                description: Some(
                    "Start fresh and run implementation without approval prompts.".into(),
                ),
            });
        } else {
            choices.push(coco_types::PermissionAskChoice {
                value: ExitPlanChoice::ClearAcceptEdits.as_str().into(),
                label: "Yes, clear context and auto-accept edits".into(),
                description: Some("Start fresh and allow file edits during implementation.".into()),
            });
        }
    }
    choices.push(coco_types::PermissionAskChoice {
        value: ExitPlanChoice::KeepAcceptEdits.as_str().into(),
        label: if ctx.permission_context.bypass_available {
            "Yes, and bypass permissions".into()
        } else {
            "Yes, auto-accept edits".into()
        },
        description: Some("Keep this conversation and proceed with elevated edit approval.".into()),
    });
    choices.push(coco_types::PermissionAskChoice {
        value: ExitPlanChoice::KeepDefault.as_str().into(),
        label: "Yes, manually approve edits".into(),
        description: Some("Keep this conversation and ask before file edits.".into()),
    });
    choices.push(coco_types::PermissionAskChoice {
        value: ExitPlanChoice::No.as_str().into(),
        label: "No, keep planning".into(),
        description: None,
    });
    choices
}

fn allowed_prompts_for_detail(input: &ExitPlanModeInput) -> Vec<PermissionExitPlanAllowedPrompt> {
    input
        .allowed_prompts
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|prompt| PermissionExitPlanAllowedPrompt {
            tool: prompt.tool.as_str().to_string(),
            prompt: prompt.prompt.clone(),
        })
        .collect()
}

fn bounded_approval_feedback_suffix(feedback: Option<&str>) -> String {
    let Some(feedback) = feedback
        .map(str::trim)
        .filter(|feedback| !feedback.is_empty())
    else {
        return String::new();
    };
    let mut chars = feedback.chars();
    let bounded: String = chars
        .by_ref()
        .take(APPROVAL_FEEDBACK_CONTEXT_CHAR_LIMIT)
        .collect();
    let truncated = chars.next().is_some();
    let marker = if truncated {
        APPROVAL_FEEDBACK_TRUNCATION_MARKER
    } else {
        ""
    };
    format!("\n\nUser feedback on the approved plan:\n{bounded}{marker}")
}

#[async_trait::async_trait]
impl Tool for ExitPlanModeTool {
    type Input = ExitPlanModeInput;
    /// Output is `Value` — `ExitPlanModeOutput` is rich and serializes
    /// cleanly for the model render path.
    type Output = Value;

    fn runtime_validation_schema(&self) -> &coco_tool_runtime::ToolInputSchema {
        static SCHEMA: std::sync::OnceLock<coco_tool_runtime::ToolInputSchema> =
            std::sync::OnceLock::new();
        SCHEMA.get_or_init(|| {
            let mut schema = coco_tool_runtime::derive_input_schema_value::<ExitPlanModeInput>();
            if let Some(obj) = schema.as_object_mut() {
                obj.remove("additionalProperties");
            }
            coco_tool_runtime::ToolInputSchema::from_static_value(schema)
        })
    }

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::ExitPlanMode)
    }
    fn name(&self) -> &str {
        ToolName::ExitPlanMode.as_str()
    }

    /// Model-facing spec exposes only `allowedPrompts`. The
    /// `allowedPrompts` item shape (`{ tool: enum["Bash"], prompt }`, both
    /// required) is derived from [`ExitPlanAllowedPrompt`]. The runtime schema
    /// intentionally permits additional passthrough fields, mirroring TS while
    /// typed execution consumes only `allowedPrompts`.
    async fn tool_spec(
        &self,
        _ctx: &coco_tool_runtime::SchemaContext,
        prompt_opts: &coco_tool_runtime::PromptOptions,
    ) -> coco_tool_runtime::ToolSpec {
        coco_tool_runtime::ToolSpec::Function(coco_tool_runtime::FunctionToolSpec {
            name: self.name().to_string(),
            description: self.prompt(prompt_opts).await,
            parameters: self.runtime_validation_schema().as_value().clone(),
            strict: self.strict(),
        })
    }

    fn description(&self, _input: &ExitPlanModeInput, _options: &DescriptionOptions) -> String {
        "Prompts the user to exit plan mode and start coding".into()
    }
    async fn prompt(&self, _options: &PromptOptions) -> String {
        exit_plan_mode_prompt()
    }
    fn user_facing_name(&self) -> &str {
        ""
    }
    fn is_read_only(&self, _input: &ExitPlanModeInput) -> bool {
        false
    }
    fn is_concurrency_safe(&self, _input: &ExitPlanModeInput) -> bool {
        true
    }
    // Not deferred (trait default `false`) — kept loaded alongside
    // `EnterPlanModeTool` so neither plan-mode tool needs a `ToolSearch` hop.
    // `search_hint` kept but inert while non-deferred (ready if re-deferred).
    fn search_hint(&self) -> Option<&str> {
        Some("present plan for approval and start coding (plan mode only)")
    }
    /// Gated by the `plan_mode` feature (default on); hidden when the user
    /// opts out via `features.plan_mode = false`. Paired with
    /// `EnterPlanModeTool` so neither half of the subsystem leaks through.
    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.features.enabled(coco_types::Feature::PlanMode)
    }

    /// Reject if not currently in plan mode.
    ///
    /// Teammates bypass validation — their AppState may show the leader's mode,
    /// so `isPlanModeRequired()` is the real source of truth for teammates.
    fn validate_input(&self, _input: &ExitPlanModeInput, ctx: &ToolUseContext) -> ValidationResult {
        // Teammates always pass validation.
        // Note: agent_id.is_some() is NOT the same as isTeammate().
        // Regular subagents have agent_id but are NOT teammates.
        if ctx.is_teammate {
            return ValidationResult::Valid;
        }

        if ctx.permission_context.mode != PermissionMode::Plan {
            return ValidationResult::invalid_with_code(
                "You are not in plan mode. This tool is only for exiting plan mode \
                 after writing a plan. If your plan was already approved, continue \
                 with implementation.",
                "1",
            );
        }
        ValidationResult::Valid
    }

    /// Non-teammate contexts require user confirmation to exit plan mode.
    ///
    /// Teammates bypass the permission UI entirely. The call() method handles
    /// their behavior: isPlanModeRequired() sends plan_approval_request to
    /// leader, otherwise exits locally.
    async fn check_permissions(
        &self,
        input: &ExitPlanModeInput,
        ctx: &ToolUseContext,
    ) -> coco_types::ToolCheckResult {
        // Teammates bypass the permission UI entirely.
        // Use explicit Allow rather than Passthrough — we want the
        // evaluator to short-circuit on this positive opinion before
        // rule / mode-fallthrough lookups.
        if ctx.is_teammate {
            return coco_types::ToolCheckResult::Allow {
                updated_input: None,
                feedback: None,
            };
        }
        let state = self.derive_state(input, ctx, None).await;
        let choices = build_exit_plan_mode_choices_for_state(&state, ctx);
        let detail = Some(PermissionRequestDetail::ExitPlanMode {
            outcome: state.outcome,
            plan: state.plan,
            plan_file_path: state.plan_file_path,
            allowed_prompts: allowed_prompts_for_detail(input),
        });
        // Non-teammates: require user confirmation. The interactive TUI
        // renders ExitPlanMode with a dedicated approval prompt and option
        // set; SDK/headless clients keep the plain Ask shape.
        coco_types::ToolCheckResult::Ask {
            message: "Exit plan mode?".into(),
            suggestions: vec![],
            choices: Some(choices),
            detail,
        }
    }

    async fn execute(
        &self,
        input: ExitPlanModeInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<Value>, ToolError> {
        let is_agent = ctx.agent_id.is_some();
        let agent_id_str = ctx.agent_id.as_ref().map(|a| a.as_str().to_string());

        let approval_detail = ctx.permission_resolution_detail.clone();
        let choice = approval_detail
            .as_ref()
            .map(|coco_types::PermissionResolutionDetail::ExitPlanMode { choice, .. }| *choice);
        let approved_plan_edit = self.approved_plan_edit_from_detail(ctx, approval_detail)?;
        let state = self.derive_state(&input, ctx, approved_plan_edit).await;
        let outcome = state.outcome;
        let plan = state.plan;
        let file_path = state.plan_file_path;
        let input_plan_is_edit = state.plan_was_edited;
        let has_implementation_plan = outcome.has_implementation_plan();
        let result_file_path = has_implementation_plan.then(|| file_path.clone()).flatten();

        if input_plan_is_edit
            && has_implementation_plan
            && let (Some(plan_content), Some(path)) = (&plan, &file_path)
            && let Err(e) = tokio::fs::write(path, plan_content.as_bytes()).await
        {
            tracing::warn!("Failed to persist edited plan to {path}: {e}");
        }

        let has_task_tool = ctx
            .tools
            .get_by_name(ToolName::TeamCreate.as_str())
            .is_some_and(|t| t.is_enabled(ctx));

        // ── Teammate branch — write plan_approval_request to leader inbox ──
        //
        // If the caller is a teammate whose role requires plan approval, we
        // don't let them exit locally — the plan is serialized and handed off
        // to the team lead via mailbox. The leader sees it, decides, and writes
        // a plan_approval_response back to this teammate's inbox.
        //
        // Gate on `is_teammate && plan_mode_required`. Voluntary teammates
        // (is_teammate=true but plan_mode_required=false) fall through to the
        // normal exit path — they restore their mode locally just like a
        // non-swarm session.
        if ctx.is_teammate && ctx.plan_mode_required {
            let Some(plan_text) = plan.as_deref() else {
                return Err(ToolError::InvalidInput {
                    message: format!(
                        "No plan file found at {}. Please write your plan to this file before calling ExitPlanMode.",
                        file_path.as_deref().unwrap_or("(unresolved)"),
                    ),
                    error_code: Some("1".into()),
                });
            };
            // Swarm identity is pre-resolved by the engine into
            // `ctx.agent_name` + `ctx.team_name` (3-tier fallback done
            // once at ctx build time). Tools read from the typed field,
            // never from process env directly.
            let agent_name = ctx
                .agent_name
                .clone()
                .or_else(|| agent_id_str.clone())
                .unwrap_or_else(|| "unknown".into());
            let team_name = ctx.team_name.clone().unwrap_or_else(|| "default".into());

            let timestamp = chrono::Utc::now().to_rfc3339();
            let short_uuid: String = uuid::Uuid::new_v4()
                .simple()
                .to_string()
                .chars()
                .take(8)
                .collect();
            let request_id = format!("plan_approval-{agent_name}-{team_name}-{short_uuid}");

            // Typed protocol message (wire shape preserved via serde
            // renames). Shared schema in `coco_tool_runtime::plan_approval`.
            let approval_msg = PlanApprovalMessage::PlanApprovalRequest(PlanApprovalRequest {
                from: agent_name.clone(),
                timestamp: timestamp.clone(),
                plan_file_path: file_path.clone().unwrap_or_default(),
                plan_content: plan_text.to_string(),
                request_id: request_id.clone(),
            });
            let serialized =
                serde_json::to_string(&approval_msg).map_err(|e| ToolError::ExecutionFailed {
                    message: format!("failed to serialize plan_approval_request: {e}"),
                    display_data: None,
                    source: None,
                })?;
            let envelope = coco_tool_runtime::MailboxEnvelope {
                text: serialized,
                from: agent_name.clone(),
                timestamp: timestamp.clone(),
            };
            // "team-lead" is the canonical inbox name.
            ctx.mailbox
                .write_to_mailbox("team-lead", &team_name, envelope)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    message: format!(
                        "failed to write plan_approval_request to leader mailbox: {e}"
                    ),
                    display_data: None,
                    source: None,
                })?;

            // Queue the "awaiting approval" flags — the teammate
            // stays in plan mode until the leader responds.
            let request_id_for_patch = request_id.clone();
            let awaiting_patch: coco_types::AppStatePatch = Box::new(move |state| {
                state.awaiting_plan_approval = true;
                state.awaiting_plan_approval_request_id = Some(request_id_for_patch);
            });

            let out = ExitPlanModeOutput {
                outcome,
                plan: Some(plan_text.to_string()),
                is_agent: true,
                file_path,
                awaiting_leader_approval: true,
                request_id: Some(request_id),
                ..Default::default()
            };
            let display_data = exit_plan_mode_display_data(&out);
            return Ok(
                ToolResult::data(serde_json::to_value(&out).unwrap_or_default())
                    .with_display_data(display_data)
                    .with_patch(awaiting_patch),
            );
        }

        // All mode-related writes happen here: flips mode → restoreMode,
        // clears prePlanMode, toggles strippedDangerousRules, and sets the
        // exit banner latches.
        //
        // Source of truth: `app_state.pre_plan_mode` (set by
        // EnterPlanMode.execute). Fall back to `ctx.permission_context`
        // for callers without app_state.
        //
        // Auto-mode-exit banner: fires when auto was effectively active
        // during the plan but we're not restoring to Auto. In Rust
        // "auto was active" = `stripped_dangerous_rules.is_some()` OR
        // `pre_plan_mode == Some(Auto)`.
        let (pre_plan_from_state, stripped_from_state) = match ctx.app_state.as_ref() {
            Some(state) => {
                let guard = state.read().await;
                (
                    guard.permissions.pre_plan_mode,
                    guard.permissions.stripped_dangerous_rules.is_some(),
                )
            }
            None => (
                ctx.permission_context.pre_plan_mode,
                ctx.permission_context.stripped_dangerous_rules.is_some(),
            ),
        };
        let restore_mode = if has_implementation_plan {
            match choice {
                Some(ExitPlanChoice::ClearBypassPermissions)
                    if ctx.permission_context.bypass_available =>
                {
                    PermissionMode::BypassPermissions
                }
                Some(ExitPlanChoice::ClearBypassPermissions | ExitPlanChoice::ClearAcceptEdits) => {
                    PermissionMode::AcceptEdits
                }
                Some(ExitPlanChoice::KeepAcceptEdits)
                    if ctx.permission_context.bypass_available =>
                {
                    PermissionMode::BypassPermissions
                }
                Some(ExitPlanChoice::KeepAcceptEdits) => PermissionMode::AcceptEdits,
                Some(ExitPlanChoice::KeepDefault) => PermissionMode::Default,
                // `No` is a denial routed by the TUI before `execute`; if it ever
                // arrives here, treat it like an absent choice — restore pre-plan.
                Some(ExitPlanChoice::No) | None => {
                    pre_plan_from_state.unwrap_or(PermissionMode::Default)
                }
            }
        } else {
            pre_plan_from_state.unwrap_or(PermissionMode::Default)
        };
        let auto_was_active_during_plan =
            stripped_from_state || pre_plan_from_state == Some(PermissionMode::Auto);
        let restoring_to_auto = restore_mode == PermissionMode::Auto;
        let needs_auto_exit = auto_was_active_during_plan && !restoring_to_auto;

        // Pre-compute the strip-snapshot (if we'll enter Auto) before
        // building the patch closure. `strip_dangerous_rules`
        // operates on a `ToolPermissionContext` and returns the
        // stashed rules via the `stripped_dangerous_rules` field; we
        // snapshot here so the closure only needs to store the
        // resulting `Option<PermissionRulesBySource>`.
        let snapshotted_stripped_rules = if restoring_to_auto && !stripped_from_state {
            let mut shadow_ctx = ctx.permission_context.clone();
            coco_permissions::dangerous_rules::strip_dangerous_rules(
                &mut shadow_ctx,
                /*is_ant_user*/ false,
            );
            shadow_ctx.stripped_dangerous_rules
        } else {
            None
        };

        // Clear-context options schedule a history clear plus a fresh
        // implementation user message for the next turn.
        let clear_history_requested =
            has_implementation_plan && choice.is_some_and(ExitPlanChoice::clears_context);
        tracing::info!(
            ?choice,
            has_implementation_plan,
            clear_history_requested,
            ?restore_mode,
            "ExitPlanMode resolved approval choice",
        );
        let post_clear_message = (clear_history_requested && has_implementation_plan).then(|| {
            let transcript_hint = ctx
                .transcript_path
                .as_ref()
                .map(|path| {
                    format!(
                        "\n\nTranscript for pre-clear planning context: {}",
                        path.display()
                    )
                })
                .unwrap_or_default();
            let plan_file_hint = file_path
                .as_deref()
                .map(|path| format!("\n\nPlan file path: {path}"))
                .unwrap_or_default();
            let team_hint = if has_task_tool {
                "\n\nIf this plan can be broken down into multiple independent tasks, consider using TeamCreate to parallelize the work."
            } else {
                ""
            };
            let feedback_suffix = bounded_approval_feedback_suffix(ctx.approval_feedback.as_deref());
            format!(
                "Implement the following plan:\n\n{}{plan_file_hint}{transcript_hint}{team_hint}{feedback_suffix}",
                plan.as_deref().unwrap_or_default(),
            )
        });
        let pending_verification_plan = if ctx.plan_verify_execution {
            plan.as_ref().filter(|p| !p.trim().is_empty()).cloned()
        } else {
            None
        };

        // Queue the full ExitPlanMode transition.
        let patch: coco_types::AppStatePatch = Box::new(move |state| {
            state.permissions.mode = Some(restore_mode);
            state.permissions.pre_plan_mode = None;
            state.has_exited_plan_mode = true;
            state.needs_plan_mode_exit_attachment = true;
            state.pending_plan_mode_exit_outcome = Some(outcome);
            state.pending_plan_verification = pending_verification_plan
                .clone()
                .map(PendingPlanVerificationState::new);
            if needs_auto_exit {
                state.needs_auto_mode_exit_attachment = true;
            }
            // Dangerous-rules stash management on Auto boundary.
            // (The strip happens on Plan→Auto, restore on
            // Auto→non-Auto exit path.)
            if restoring_to_auto && state.permissions.stripped_dangerous_rules.is_none() {
                state.permissions.stripped_dangerous_rules = snapshotted_stripped_rules;
            } else if !restoring_to_auto && state.permissions.stripped_dangerous_rules.is_some() {
                state.permissions.stripped_dangerous_rules = None;
            }
            if clear_history_requested {
                state.pending_clear_message_history = true;
                state.pending_plan_implementation_message = post_clear_message;
            }
        });

        let out = ExitPlanModeOutput {
            outcome,
            plan,
            is_agent,
            file_path: result_file_path,
            has_task_tool: if has_task_tool { Some(true) } else { None },
            plan_was_edited: if has_implementation_plan && input_plan_is_edit {
                Some(true)
            } else {
                None
            },
            ..Default::default()
        };
        let display_data = exit_plan_mode_display_data(&out);
        Ok(
            ToolResult::data(serde_json::to_value(&out).unwrap_or_default())
                .with_display_data(display_data)
                .with_patch(patch),
        )
    }

    fn render_for_model(&self, data: &Value) -> Vec<ToolResultContentPart> {
        // The four-variant formatting (teammate-awaiting / sub-agent / empty /
        // approved) lives in [`ExitPlanModeTool::build_instructions`]; this is
        // the trait hook that wires it up to the executor's render path.
        vec![ToolResultContentPart::Text {
            text: Self::build_instructions(data),
            provider_options: None,
        }]
    }
}

fn exit_plan_mode_display_data(out: &ExitPlanModeOutput) -> ToolDisplayData {
    ToolDisplayData::ExitPlanModeResult(ExitPlanModeResult {
        outcome: out.outcome,
        plan: out.plan.clone().unwrap_or_default(),
        file_path: out.file_path.clone(),
        awaiting_leader_approval: out.awaiting_leader_approval,
        is_agent: out.is_agent,
        plan_was_edited: out.plan_was_edited.unwrap_or(false),
    })
}

impl ExitPlanModeTool {
    fn approved_plan_edit_from_detail(
        &self,
        ctx: &ToolUseContext,
        detail: Option<coco_types::PermissionResolutionDetail>,
    ) -> Result<Option<String>, ToolError> {
        let Some(coco_types::PermissionResolutionDetail::ExitPlanMode { edited_plan, .. }) = detail
        else {
            return Ok(None);
        };
        let Some(edited_plan) = edited_plan.filter(|plan| !plan.trim().is_empty()) else {
            return Ok(None);
        };

        let plans_dir = ctx.plans_dir.clone().or_else(|| {
            ctx.config_home
                .as_ref()
                .map(|ch| coco_context::resolve_plans_directory(ch, None, None))
        });
        if ctx.session_id_for_history.is_some() && plans_dir.is_some() {
            return Ok(Some(edited_plan));
        }

        Err(ToolError::InvalidInput {
            message: "ExitPlanMode approved edit requires a resolved plan file path".into(),
            error_code: Some("approved_edit_without_plan_path".into()),
        })
    }

    async fn derive_state(
        &self,
        _input: &ExitPlanModeInput,
        ctx: &ToolUseContext,
        edited_plan: Option<String>,
    ) -> coco_context::ExitPlanModeDerivedState {
        let agent_id = ctx.agent_id.as_ref().map(coco_types::AgentId::as_str);
        let plans_dir = ctx.plans_dir.clone().or_else(|| {
            ctx.config_home
                .as_ref()
                .map(|ch| coco_context::resolve_plans_directory(ch, None, None))
        });
        let entry_ms = match ctx.app_state.as_ref() {
            Some(state) => state.read().await.plan_mode_entry_ms,
            None => None,
        };
        coco_context::derive_exit_plan_mode_state(
            ctx.session_id_for_history.as_deref(),
            plans_dir.as_deref(),
            agent_id,
            entry_ms,
            edited_plan,
        )
    }

    /// Build the instructions text returned to the model as tool_result content.
    pub fn build_instructions(result_data: &Value) -> String {
        let out: ExitPlanModeOutput =
            serde_json::from_value(result_data.clone()).unwrap_or_default();

        // Teammate "awaiting approval" branch.
        if out.awaiting_leader_approval {
            let request_id = out.request_id.as_deref().unwrap_or("(unknown)");
            let file_path_str = out.file_path.as_deref().unwrap_or("(unknown)");
            return format!(
                "Your plan has been submitted to the team lead for approval.\n\n\
                 Plan file: {file_path_str}\n\n\
                 **What happens next:**\n\
                 1. Wait for the team lead to review your plan\n\
                 2. You will receive a message in your inbox with approval/rejection\n\
                 3. If approved, you can proceed with implementation\n\
                 4. If rejected, refine your plan based on the feedback\n\n\
                 **Important:** Do NOT proceed until you receive approval. Check \
                 your inbox for response.\n\n\
                 Request ID: {request_id}"
            );
        }

        if out.is_agent {
            return "User has approved the plan. There is nothing else needed \
                    from you now. Please respond with \"ok\""
                .to_string();
        }

        if out.outcome == ExitPlanModeOutcome::NoImplementationPlan {
            return "User has approved exiting plan mode. There is no implementation plan to execute."
                .to_string();
        }
        let plan_text = out.plan.as_deref().unwrap_or("");

        let team_hint = if out.has_task_tool.unwrap_or(false) {
            "\n\nIf this plan can be broken down into multiple independent tasks, \
             consider using the TeamCreate tool to create a team and parallelize \
             the work."
        } else {
            ""
        };
        let plan_label = if out.plan_was_edited.unwrap_or(false) {
            "Approved Plan (edited by user)"
        } else {
            "Approved Plan"
        };
        let path_note = out
            .file_path
            .as_deref()
            .map(|p| {
                format!(
                    "\nYour plan has been saved to: {p}\n\
                     You can refer back to it if needed during implementation."
                )
            })
            .unwrap_or_default();

        format!(
            "User has approved your plan. You can now start coding. \
             Start with updating your todo list if applicable\n\
             {path_note}{team_hint}\n\n\
             ## {plan_label}:\n{plan_text}"
        )
    }
}

#[cfg(test)]
#[path = "plan_mode.test.rs"]
mod tests;
