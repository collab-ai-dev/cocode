//! RTK external-tier command resolution for `BashTool` (design §4.2, phase 1).
//!
//! Lives in a sibling module so the (already large) `bash.rs` isn't extended,
//! and so the `RTK_TOOL_DESCRIPTION_SUFFIX` note documents itself rather than
//! stealing `BASH_TOOL_DESCRIPTION`'s doc comment.
//!
//! The rewrite runs **after** permission / security / read-only / sandbox
//! judgment (all of which evaluate the ORIGINAL command) and swaps only the
//! string that is actually spawned. Passthrough / feature-off leaves the
//! command untouched — the result is then byte-identical to the pre-rtk path.

use coco_tool_runtime::ToolUseContext;
use serde_json::Value;

/// Appended to the Bash tool description when an RTK rewriter is wired for the
/// session, giving the model a documented per-command opt-out.
pub(super) const RTK_TOOL_DESCRIPTION_SUFFIX: &str = "\n\nDev-tool command output is compressed by rtk; prefix a command with `RTK_DISABLED=1` to get raw output.";

/// The command to spawn plus the provenance recorded in the Bash result
/// envelope. The two states are exclusive by construction — a passthrough
/// carries no rtk fields, a rewrite always carries both `original` and `exec` —
/// so the envelope can never be half-annotated.
pub(super) enum ResolvedCommand {
    /// rtk off / passthrough: the spawned string IS the model-issued command,
    /// and no `rtk` / `rtkCommand` envelope fields are stamped.
    Passthrough(String),
    /// External-tier rewrite fired: spawn `exec`, but keep `original` as the
    /// envelope `command` (so `render_for_model`'s exit-code interpretation
    /// stays command-aware) and for progress display + hint attribution.
    Rewritten { original: String, exec: String },
}

impl ResolvedCommand {
    /// The string actually spawned — rewritten in the external tier, else the
    /// original.
    pub(super) fn exec(&self) -> &str {
        match self {
            ResolvedCommand::Passthrough(c) => c,
            ResolvedCommand::Rewritten { exec, .. } => exec,
        }
    }

    /// The model-issued command (envelope `command`, progress, hint attribution).
    pub(super) fn original(&self) -> &str {
        match self {
            ResolvedCommand::Passthrough(c) => c,
            ResolvedCommand::Rewritten { original, .. } => original,
        }
    }

    /// True when the external rewrite fired (the spawned command is rtk-wrapped).
    pub(super) fn was_rewritten(&self) -> bool {
        matches!(self, ResolvedCommand::Rewritten { .. })
    }

    /// Stamp `rtk` / `rtkCommand` provenance onto the result envelope. No-op on
    /// passthrough.
    pub(super) fn annotate_envelope(&self, result_obj: &mut Value) {
        if let ResolvedCommand::Rewritten { exec, .. } = self {
            result_obj["rtk"] = Value::String(coco_shell::RtkTier::External.as_str().to_string());
            result_obj["rtkCommand"] = Value::String(exec.clone());
        }
    }
}

/// Stamp builtin-tier (`rtk = "builtin"`) provenance when the phase-2 post-exec
/// filter compressed the output. Mutually exclusive with
/// [`ResolvedCommand::annotate_envelope`]'s external stamp — the two tiers never
/// both act on one call (§3.5 no-double-filtering), so this only ever runs on a
/// [`ResolvedCommand::Passthrough`].
pub(super) fn annotate_builtin_tier(result_obj: &mut Value) {
    result_obj["rtk"] = Value::String(coco_shell::RtkTier::Builtin.as_str().to_string());
}

/// Apply the RTK external-tier rewrite (phase 1) when `Feature::OutputRewrite` supplied a
/// rewriter on the context. Infallible — every rtk failure maps to passthrough.
///
/// `sandbox_active` is `true` for any non-bypassed sandbox session: rtk rewriting
/// can turn a sandbox-excluded command (`git`) into a non-excluded one
/// (`rtk git`) that the executor would then wrap, and rtk's SQLite history
/// write fails wrapped — so rtk is skipped for the whole sandboxed session
/// (design §4.3), decided here before spawn.
pub(super) async fn resolve_rtk_command(
    command: &str,
    run_in_background: bool,
    sandbox_active: bool,
    ctx: &ToolUseContext,
) -> ResolvedCommand {
    let Some(rewriter) = ctx.output_rewriter.as_ref() else {
        return ResolvedCommand::Passthrough(command.to_string());
    };
    // Builtin-tier modes (`BuiltinFirst` / `BuiltinOnly`) compress post-exec and
    // never touch the spawned string — skip the rewrite entirely so the original
    // command runs unmodified (and the post-exec filter handles compression).
    if !rewriter.does_pre_spawn_rewrite() {
        return ResolvedCommand::Passthrough(command.to_string());
    }
    let site = coco_shell::RewriteSite {
        background: run_in_background,
        sandboxed: sandbox_active,
    };
    match rewriter.rewrite(command, site).await {
        coco_shell::RewriteOutcome::Rewritten(exec) => ResolvedCommand::Rewritten {
            original: command.to_string(),
            exec,
        },
        coco_shell::RewriteOutcome::Passthrough(_) => {
            ResolvedCommand::Passthrough(command.to_string())
        }
    }
}

/// Phase-2 builtin-tier post-exec compression (design §3.3). Returns the
/// compressed stdout when the embedded filter fired, else `None` (keep raw).
///
/// Gated on the rewriter's `does_post_exec_filter` capability and the
/// no-double-filtering rule (`!cmd.was_rewritten()`), so it never runs when the
/// external rewrite already compressed this call. Unlike the pre-spawn rewrite
/// it is *not* sandbox- or background-gated: the filter is a pure in-process
/// text transform with no subprocess and no SQLite write, so it works in both.
/// Infallible — a filter failure or panic degrades to `None`.
pub(super) async fn apply_post_exec_filter(
    ctx: &ToolUseContext,
    cmd: &ResolvedCommand,
    exit_code: i32,
    stdout: &str,
) -> Option<String> {
    let rewriter = ctx.output_rewriter.as_ref()?;
    if cmd.was_rewritten() || !rewriter.does_post_exec_filter() {
        return None;
    }
    rewriter
        .filter_output(cmd.original(), exit_code, stdout)
        .await
}

#[cfg(test)]
#[path = "bash_rtk.test.rs"]
mod tests;
