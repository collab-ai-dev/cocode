use super::*;
use coco_tool_runtime::ToolUseContext;
use serde_json::json;
use std::sync::Arc;

/// Configurable [`coco_shell::BashOutputRewriter`] double so the tier-arbitration
/// logic (`resolve_rtk_command` pre-spawn gate + `apply_post_exec_filter`
/// capability / no-double-filter gate) can be exercised without a real rtk
/// backend.
#[derive(Debug)]
struct MockRewriter {
    pre_spawn: bool,
    post_exec: bool,
    rewrite_outcome: coco_shell::RewriteOutcome,
    filter_result: Option<String>,
}

#[async_trait::async_trait]
impl coco_shell::BashOutputRewriter for MockRewriter {
    async fn rewrite(
        &self,
        _command: &str,
        _site: coco_shell::RewriteSite,
    ) -> coco_shell::RewriteOutcome {
        self.rewrite_outcome.clone()
    }
    fn does_pre_spawn_rewrite(&self) -> bool {
        self.pre_spawn
    }
    fn does_post_exec_filter(&self) -> bool {
        self.post_exec
    }
    async fn filter_output(
        &self,
        _command: &str,
        _exit_code: i32,
        _stdout: &str,
    ) -> Option<String> {
        self.filter_result.clone()
    }
}

fn ctx_with(rewriter: MockRewriter) -> ToolUseContext {
    let mut ctx = ToolUseContext::test_default();
    ctx.output_rewriter = Some(Arc::new(rewriter));
    ctx
}

fn passthrough_outcome() -> coco_shell::RewriteOutcome {
    coco_shell::RewriteOutcome::Passthrough(coco_shell::PassthroughReason::BinaryMissing)
}

#[test]
fn passthrough_exposes_original_as_exec_and_stamps_nothing() {
    let cmd = ResolvedCommand::Passthrough("git status".to_string());
    assert_eq!(cmd.exec(), "git status");
    assert_eq!(cmd.original(), "git status");
    assert!(!cmd.was_rewritten());

    let mut obj = json!({ "command": "git status" });
    cmd.annotate_envelope(&mut obj);
    assert!(obj.get("rtk").is_none());
    assert!(obj.get("rtkCommand").is_none());
}

#[test]
fn rewritten_keeps_original_and_stamps_provenance() {
    let cmd = ResolvedCommand::Rewritten {
        original: "git status".to_string(),
        exec: "rtk git status".to_string(),
    };
    assert_eq!(cmd.exec(), "rtk git status");
    assert_eq!(cmd.original(), "git status");
    assert!(cmd.was_rewritten());

    let mut obj = json!({ "command": "git status" });
    cmd.annotate_envelope(&mut obj);
    // The envelope keeps the model-issued command; provenance is additive.
    assert_eq!(obj["command"], "git status");
    assert_eq!(obj["rtk"], "external");
    assert_eq!(obj["rtkCommand"], "rtk git status");
}

#[tokio::test]
async fn resolve_passthrough_when_no_rewriter() {
    // `ToolUseContext::test_default()` leaves `rtk = None` (feature off), so a
    // command resolves to a passthrough with no rtk envelope fields.
    let ctx = ToolUseContext::test_default();
    let resolved = resolve_rtk_command(
        "git status",
        /*run_in_background*/ false,
        /*sandbox_active*/ false,
        &ctx,
    )
    .await;
    assert!(!resolved.was_rewritten());
    assert_eq!(resolved.exec(), "git status");
    assert_eq!(resolved.original(), "git status");
}

#[test]
fn builtin_tier_stamps_rtk_builtin_without_rtk_command() {
    // Post-exec (builtin) compression records the tier but no rewritten string —
    // the executed command was the original.
    let mut obj = json!({ "command": "df -h" });
    annotate_builtin_tier(&mut obj);
    assert_eq!(obj["rtk"], "builtin");
    assert!(obj.get("rtkCommand").is_none());
    assert_eq!(obj["command"], "df -h");
}

#[tokio::test]
async fn post_exec_filter_is_none_without_rewriter() {
    // Feature off (`rtk = None`) → no post-exec compression, raw output kept.
    let ctx = ToolUseContext::test_default();
    let cmd = ResolvedCommand::Passthrough("df -h".to_string());
    assert_eq!(
        apply_post_exec_filter(&ctx, &cmd, /*exit_code*/ 0, "Filesystem ...").await,
        None
    );
}

#[tokio::test]
async fn post_exec_filter_returns_compressed_when_builtin_active() {
    let ctx = ctx_with(MockRewriter {
        pre_spawn: false,
        post_exec: true,
        rewrite_outcome: passthrough_outcome(),
        filter_result: Some("compressed".to_string()),
    });
    let cmd = ResolvedCommand::Passthrough("df -h".to_string());
    assert_eq!(
        apply_post_exec_filter(&ctx, &cmd, 0, "raw output").await,
        Some("compressed".to_string())
    );
}

#[tokio::test]
async fn post_exec_filter_skipped_when_command_was_rewritten() {
    // §3.5 no-double-filtering: the external rewrite already compressed this call,
    // so the post-exec filter must NOT run even though the backend can filter.
    let ctx = ctx_with(MockRewriter {
        pre_spawn: true,
        post_exec: true,
        rewrite_outcome: passthrough_outcome(),
        filter_result: Some("compressed".to_string()),
    });
    let cmd = ResolvedCommand::Rewritten {
        original: "git status".to_string(),
        exec: "rtk git status".to_string(),
    };
    assert_eq!(
        apply_post_exec_filter(&ctx, &cmd, 0, "raw output").await,
        None
    );
}

#[tokio::test]
async fn post_exec_filter_skipped_when_backend_declines_post_exec() {
    // A pre-spawn-only backend (`does_post_exec_filter() == false`) is never asked
    // to filter, even when a `filter_result` would be available.
    let ctx = ctx_with(MockRewriter {
        pre_spawn: true,
        post_exec: false,
        rewrite_outcome: passthrough_outcome(),
        filter_result: Some("unused".to_string()),
    });
    let cmd = ResolvedCommand::Passthrough("df -h".to_string());
    assert_eq!(
        apply_post_exec_filter(&ctx, &cmd, 0, "raw output").await,
        None
    );
}

#[tokio::test]
async fn resolve_skips_rewrite_for_builtin_mode() {
    // `does_pre_spawn_rewrite() == false` (BuiltinFirst/Only): the command is
    // spawned unmodified and `rewrite()` is never consulted, even though this
    // mock would otherwise return a rewrite.
    let ctx = ctx_with(MockRewriter {
        pre_spawn: false,
        post_exec: true,
        rewrite_outcome: coco_shell::RewriteOutcome::Rewritten("SHOULD NOT SPAWN".to_string()),
        filter_result: None,
    });
    let resolved = resolve_rtk_command("git status", false, false, &ctx).await;
    assert!(!resolved.was_rewritten());
    assert_eq!(resolved.exec(), "git status");
}

#[tokio::test]
async fn resolve_rewrites_for_external_mode() {
    // `does_pre_spawn_rewrite() == true` (External*): the rewrite fires and the
    // envelope keeps the original command.
    let ctx = ctx_with(MockRewriter {
        pre_spawn: true,
        post_exec: false,
        rewrite_outcome: coco_shell::RewriteOutcome::Rewritten("rtk git status".to_string()),
        filter_result: None,
    });
    let resolved = resolve_rtk_command("git status", false, false, &ctx).await;
    assert!(resolved.was_rewritten());
    assert_eq!(resolved.exec(), "rtk git status");
    assert_eq!(resolved.original(), "git status");
}
