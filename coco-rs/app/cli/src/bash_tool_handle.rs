//! Session-backed [`BashToolHandle`] — routes in-prompt skill / slash-command
//! shell markers through the real `Bash` tool with a per-command permission
//! check.
//!
//! For each marker, calls `hasPermissionsToUseTool(BashTool, { command }, ctx)`
//! and then executes the command. The skill loader injects the skill
//! frontmatter `allowed-tools` into the permission context for the duration of
//! that call. Slash commands pass an empty `allowed_tools`.
//!
//! ## Why a snapshot `ToolUseContext`
//!
//! The canonical per-tool `ToolUseContext` is built by app/query's
//! `ToolContextFactory`, which is `pub(crate)` to that crate. Rather than
//! widen that seam, the handle is constructed with a `ToolUseContext`
//! snapshot taken at session bootstrap (the type is `Clone`). The snapshot
//! carries the resolved tool config, sandbox state, permission context, and
//! cwd cell, which is everything `BashTool::execute` + the permission
//! evaluator need. Permission *mode* is read from the snapshot, so a mode
//! that changed after bootstrap (e.g. entering Plan mode) is not reflected
//! here — acceptable for the in-prompt shell path, which only ever runs the
//! author-declared `allowed-tools` (skills) or configured rules (commands).

use std::sync::Arc;

use async_trait::async_trait;
use coco_commands::BashToolHandle;
use coco_permissions::PermissionEvaluator;
use coco_permissions::rule_compiler;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolUseContext;
use coco_tools::tools::bash::BashInput;
use coco_tools::tools::bash::BashTool;
use coco_types::PermissionBehavior;
use coco_types::PermissionDecision;
use coco_types::PermissionRule;
use coco_types::PermissionRuleSource;
use coco_types::ToolId;
use coco_types::ToolName;
use serde_json::Value;

/// Concrete [`BashToolHandle`] backed by a session `ToolUseContext` snapshot.
pub(crate) struct SessionBashToolHandle {
    /// Base context cloned per call; permission rules are augmented with the
    /// caller's `allowed_tools` before evaluation + execution.
    base_ctx: ToolUseContext,
}

impl SessionBashToolHandle {
    pub(crate) fn new(base_ctx: ToolUseContext) -> Self {
        Self { base_ctx }
    }
}

/// Build a [`SessionBashToolHandle`] from a `ToolUseContext` snapshot,
/// returning it as a shared `BashToolHandle`. The caller installs the same
/// instance into both the command registry (slash / shell-expanding
/// prompt commands) and the skill runtime's shared cell (model-invoked +
/// fork-mode skills) so every in-prompt shell path runs through one
/// permission-checked Bash route.
///
/// `base_ctx` should be a representative per-tool `ToolUseContext` for this
/// session (resolved tool config, sandbox state, permission context, cwd
/// cell). The handle clones it per command and folds the caller's
/// `allowed_tools` into the permission rules.
pub(crate) fn build_session_bash_handle(base_ctx: ToolUseContext) -> Arc<dyn BashToolHandle> {
    Arc::new(SessionBashToolHandle::new(base_ctx))
}

#[async_trait]
impl BashToolHandle for SessionBashToolHandle {
    async fn execute_with_permissions(
        &self,
        command: &str,
        allowed_tools: &[String],
    ) -> Result<String, String> {
        let input = BashInput {
            command: command.to_string(),
            ..Default::default()
        };
        // `BashInput` is `Deserialize`-only; the evaluator only reads the
        // `command` field (via `extract_shell_command`), so build the JSON
        // view directly rather than round-tripping through Serialize.
        let input_value: Value = serde_json::json!({ "command": command });

        // Clone the base context and fold the caller's `allowed-tools` into the
        // permission context as `Command`-source allow rules.
        let mut ctx = self.base_ctx.clone();
        if !allowed_tools.is_empty() {
            let rules: Vec<PermissionRule> = allowed_tools
                .iter()
                .map(|rule_str| PermissionRule {
                    source: PermissionRuleSource::Command,
                    behavior: PermissionBehavior::Allow,
                    value: rule_compiler::parse_rule_string(rule_str),
                })
                .collect();
            ctx.permission_context
                .allow_rules
                .entry(PermissionRuleSource::Command)
                .or_default()
                .extend(rules);
        }

        // Evaluate with the tool's own permission check (Bash subcommand
        // analysis) wired in. Anything other than `Allow` aborts the whole
        // expansion (MalformedCommandError).
        //
        // `check_permissions` is async but the evaluator's `ToolCheckFn` is
        // sync, so we compute the tool-level result up front and hand the
        // evaluator a closure that just returns the precomputed value.
        let tool_check = bash_check(&input, &ctx).await;
        let permission_ctx = ctx.permission_context.clone();
        let check_fn = move |_tool_id: &ToolId, _value: &Value, _pctx: &_| tool_check.clone();
        let decision = PermissionEvaluator::evaluate_with_tool_check(
            &ToolId::Builtin(ToolName::Bash),
            &input_value,
            &permission_ctx,
            Some(&check_fn),
        );

        match decision {
            PermissionDecision::Allow { .. } => {}
            PermissionDecision::Deny { message, .. } => {
                return Err(format!("permission denied: {message}"));
            }
            PermissionDecision::Ask { message, .. } => {
                // In-prompt shell cannot interactively prompt — fail closed.
                return Err(format!("permission required (not auto-allowed): {message}"));
            }
            PermissionDecision::Abort { message, .. } => {
                return Err(format!("permission aborted: {message}"));
            }
        }

        // Permission granted — run through the real Bash tool.
        match BashTool.execute(input, &ctx).await {
            Ok(result) => Ok(format_bash_output(&result.data)),
            Err(e) => Err(format!("shell command failed: {e}")),
        }
    }
}

/// Run the Bash tool's own typed `check_permissions` (subcommand analysis),
/// returning the `ToolCheckResult` the evaluator consumes. Computed up front
/// because the evaluator's `ToolCheckFn` is sync but the tool method is async.
async fn bash_check(input: &BashInput, ctx: &ToolUseContext) -> coco_types::ToolCheckResult {
    BashTool.check_permissions(input, ctx).await
}

/// Format the Bash result `data` block (`{ stdout, stderr, .. }`) into the
/// string substituted back into the prompt: trimmed stdout, then a
/// `[stderr]\n…` block when stderr is non-empty.
fn format_bash_output(data: &Value) -> String {
    let stdout = data.get("stdout").and_then(Value::as_str).unwrap_or("");
    let stderr = data.get("stderr").and_then(Value::as_str).unwrap_or("");
    let mut parts: Vec<String> = Vec::new();
    if !stdout.trim().is_empty() {
        parts.push(stdout.trim().to_string());
    }
    if !stderr.trim().is_empty() {
        parts.push(format!("[stderr]\n{}", stderr.trim()));
    }
    parts.join("\n")
}
