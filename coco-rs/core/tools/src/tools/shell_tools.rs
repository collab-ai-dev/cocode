use coco_messages::ToolResult;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::InterruptBehavior;
use coco_tool_runtime::PromptOptions;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolResultContentPart;
use coco_tool_runtime::ToolUseContext;
use coco_types::Feature;
use coco_types::ToolId;
use coco_types::ToolName;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

// ── SleepTool ──

/// Typed input for [`SleepTool`].
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct SleepInput {
    /// Number of seconds to sleep. Defaults to `1.0` when omitted.
    /// Capped at 300 seconds (5 minutes) to prevent indefinite
    /// blocking.
    #[serde(default)]
    pub seconds: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SleepOutput {
    /// Human-readable confirmation message.
    pub message: String,
    /// Actual seconds slept (post-cap).
    pub seconds: f64,
}

/// Model-facing Sleep tool description. Surfaced to the model
/// via [`SleepTool::prompt`].
const SLEEP_TOOL_PROMPT: &str = "Wait for a specified duration. The user can interrupt the sleep at any time.

Use this when the user tells you to sleep or rest, when you have nothing to do, or when you're waiting for something.

You may receive <tick> prompts — these are periodic check-ins. Look for useful work to do before sleeping.

You can call this concurrently with other tools — it won't interfere with them.

Prefer this over `Bash(sleep ...)` — it doesn't hold a shell process.

Each wake-up costs an API call, but the prompt cache expires after 5 minutes of inactivity — balance accordingly.";

pub struct SleepTool;

#[async_trait::async_trait]
impl Tool for SleepTool {
    type Input = SleepInput;
    coco_tool_runtime::impl_runtime_schema!(SleepInput);
    type Output = SleepOutput;

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::Sleep)
    }
    fn name(&self) -> &str {
        ToolName::Sleep.as_str()
    }
    /// Short UI label. The long model-facing guidance lives in [`Self::prompt`].
    fn description(&self, _input: &SleepInput, _options: &DescriptionOptions) -> String {
        "Wait for a specified duration".into()
    }
    /// Model-facing tool description.
    async fn prompt(&self, _options: &PromptOptions) -> String {
        SLEEP_TOOL_PROMPT.into()
    }
    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.features.enabled(Feature::Proactive)
    }
    fn is_read_only(&self, _input: &SleepInput) -> bool {
        true
    }
    /// Pure time-passing — Plan mode keeps Sleep visible.
    fn is_always_read_only(&self) -> bool {
        true
    }
    fn is_concurrency_safe(&self, _input: &SleepInput) -> bool {
        true
    }
    fn interrupt_behavior(&self) -> InterruptBehavior {
        InterruptBehavior::Cancel
    }
    fn should_defer(&self) -> bool {
        true
    }
    fn search_hint(&self) -> Option<&str> {
        Some("pause execution for a configurable number of seconds")
    }

    fn render_for_model(&self, out: &SleepOutput) -> Vec<ToolResultContentPart> {
        vec![ToolResultContentPart::Text {
            text: out.message.clone(),
            provider_options: None,
        }]
    }

    async fn execute(
        &self,
        input: SleepInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<SleepOutput>, ToolError> {
        let seconds = input.seconds.unwrap_or(1.0);

        if seconds < 0.0 {
            return Err(ToolError::InvalidInput {
                message: "seconds must be non-negative".into(),
                error_code: None,
            });
        }

        // Cap at 5 minutes to prevent indefinite blocking
        let capped = seconds.min(300.0);
        let duration = std::time::Duration::from_secs_f64(capped);
        tokio::select! {
            () = tokio::time::sleep(duration) => {}
            () = ctx.abort.cancelled() => return Err(ToolError::Cancelled),
        }

        Ok(ToolResult {
            data: SleepOutput {
                message: format!("Slept for {capped:.1} seconds"),
                seconds: capped,
            },
            new_messages: vec![],
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}

#[cfg(test)]
mod sleep_tool_tests {
    use super::*;
    use coco_tool_runtime::TurnAbortController;

    #[test]
    fn sleep_tool_is_cancel_interruptible() {
        let tool = SleepTool;
        assert_eq!(tool.interrupt_behavior(), InterruptBehavior::Cancel);
    }

    #[tokio::test]
    async fn sleep_tool_exits_when_abort_signal_fires() {
        let tool = SleepTool;
        let turn_abort = TurnAbortController::new();
        let mut ctx = ToolUseContext::test_default();
        ctx.abort = coco_tool_runtime::ToolAbortSignal::from_turn(turn_abort.signal());

        let task = tokio::spawn(async move {
            tool.execute(
                SleepInput {
                    seconds: Some(30.0),
                },
                &ctx,
            )
            .await
        });
        turn_abort.abort(coco_types::TurnAbortReason::SubmitInterrupt);
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .expect("sleep should exit promptly after abort")
            .expect("task should not panic");

        assert!(matches!(result, Err(ToolError::Cancelled)));
    }
}

// PowerShellTool lives in `powershell_tool.rs` so the security pipeline
// (`analyze_ps_security`, `find_unsafe_type_references`, `decode_ps_output`
// in `powershell.rs`) can be wired around the pwsh subprocess. The
// support utilities existed before but weren't called from the execute
// path — the dedicated module closes that gap.
pub use super::powershell_tool::PowerShellTool;

// ── ReplTool ──

/// Typed input for [`ReplTool`].
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ReplInput {
    /// Programming language for the REPL (e.g., `python`, `node`).
    #[serde(default)]
    pub language: Option<String>,
    /// Command to execute in the REPL.
    #[serde(default)]
    pub command: Option<String>,
}

/// REPL output. Currently unused — the tool errors out at the
/// `execute` boundary. Kept typed so a future REPL backend can
/// produce structured output (stdout / language metadata / continuation
/// state) without breaking the trait.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReplOutput {
    #[serde(default)]
    pub stdout: String,
    #[serde(default)]
    pub stderr: String,
}

pub struct ReplTool;

#[async_trait::async_trait]
impl Tool for ReplTool {
    type Input = ReplInput;
    coco_tool_runtime::impl_runtime_schema!(ReplInput);
    type Output = ReplOutput;

    fn to_auto_classifier_input(&self, input: &ReplInput) -> Option<String> {
        Some(input.command.clone().unwrap_or_default())
    }

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::Repl)
    }
    fn name(&self) -> &str {
        ToolName::Repl.as_str()
    }
    fn description(&self, _input: &ReplInput, _options: &DescriptionOptions) -> String {
        "Start an interactive REPL session for a supported language.".into()
    }
    fn is_enabled(&self, _ctx: &ToolUseContext) -> bool {
        false
    }
    /// Model-facing tool description. No upstream prompt text available;
    /// returning the short label keeps the description non-empty.
    async fn prompt(&self, _options: &PromptOptions) -> String {
        "Start an interactive REPL session for a supported language.".into()
    }

    fn is_transparent_wrapper(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        _input: ReplInput,
        _ctx: &ToolUseContext,
    ) -> Result<ToolResult<ReplOutput>, ToolError> {
        Err(ToolError::ExecutionFailed {
            message: "REPL tool is not available in this context. \
                      Use the Bash tool to run language-specific commands instead \
                      (e.g., `python3 -c \"...\"` or `node -e \"...\"`)."
                .into(),
            display_data: None,
            source: None,
        })
    }
}

// `StructuredOutputTool` lives in `tools/structured_output.rs` because
// it needs a compiled `jsonschema::Validator` at construction time and
// is conditionally registered (only in non-interactive sessions when
// `--json-schema` is supplied) — it doesn't ride the same always-on
// registration path as the rest of this module.
pub use super::structured_output::StructuredOutputTool;
