// Reads the process cwd, legitimate outside session-owned code; opts out of
// the workspace-wide `std::env::current_dir` gate (clippy.toml, §6.5/D-37).
#![allow(clippy::disallowed_methods)]

use super::*;
use coco_tool_runtime::DynTool;
use coco_tool_runtime::ToolUseContext;
use coco_types::Features;
use coco_types::PermissionBehavior;
use coco_types::PermissionMode;
use coco_types::PermissionRule;
use coco_types::PermissionRuleSource;
use coco_types::PermissionRuleValue;
use coco_types::ToolCheckResult;
use coco_types::ToolOverrides;
use pretty_assertions::assert_eq;
use std::sync::Arc;

/// Source of truth for the frozen freeform tool definition below:
/// `openai/codex@279b93242cfef379e65da97e87e44b83c5934fd7`,
/// `codex-rs/core/src/tools/handlers/{apply_patch_spec.rs,apply_patch.lark}`.
/// Update this revision and the complete golden together after reviewing the
/// corresponding upstream change.
const CODEX_APPLY_PATCH_SPEC_UPSTREAM_REVISION: &str = "279b93242cfef379e65da97e87e44b83c5934fd7";
const CODEX_APPLY_PATCH_DESCRIPTION_GOLDEN: &str = "The `apply_patch` tool can be used to edit files. This is a FREEFORM tool, so do not wrap the patch in JSON.";
const CODEX_APPLY_PATCH_LARK_GRAMMAR_GOLDEN: &str = concat!(
    "start: begin_patch hunk+ end_patch\n",
    "begin_patch: \"*** Begin Patch\" LF\n",
    "end_patch: \"*** End Patch\" LF?\n",
    "\n",
    "hunk: add_hunk | delete_hunk | update_hunk\n",
    "add_hunk: \"*** Add File: \" filename LF add_line+\n",
    "delete_hunk: \"*** Delete File: \" filename LF\n",
    "update_hunk: \"*** Update File: \" filename LF change_move? change?\n",
    "\n",
    "filename: /(.+)/\n",
    "add_line: \"+\" /(.*)/ LF -> line\n",
    "\n",
    "change_move: \"*** Move to: \" filename LF\n",
    "change: (change_context | change_line)+ eof_line?\n",
    "change_context: (\"@@\" | \"@@ \" /(.+)/) LF\n",
    "change_line: (\"+\" | \"-\" | \" \") /(.*)/ LF\n",
    "eof_line: \"*** End of File\" LF\n",
    "\n",
    "%import common.LF\n",
);

#[test]
fn is_enabled_only_when_model_adds_apply_patch() {
    let tool = ApplyPatchTool::default();
    let tool: &dyn DynTool = &tool;

    // Default overrides — model does NOT add apply_patch as extra.
    let mut ctx = ToolUseContext::test_default();
    ctx.features = Arc::new(Features::with_defaults());
    ctx.tool_overrides = Arc::new(ToolOverrides::none());
    assert!(
        !tool.is_enabled(&ctx),
        "apply_patch must be hidden when the active model didn't add it"
    );

    // gpt-5-style overrides — extra: apply_patch.
    ctx.tool_overrides =
        Arc::new(ToolOverrides::default().with_extra(ToolId::Builtin(ToolName::ApplyPatch)));
    assert!(tool.is_enabled(&ctx));

    let unavailable = ApplyPatchTool::unavailable();
    assert!(
        !<ApplyPatchTool as DynTool>::is_enabled(&unavailable, &ctx),
        "model overrides must not advertise a tool the environment cannot execute"
    );
}

#[tokio::test]
async fn target_native_execution_cwd_wins_over_frontend_pathbuf() {
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(std::path::PathBuf::from("/frontend/worktree"));
    ctx.execution_cwd =
        Some(PathUri::parse("file:///C:/remote/project").expect("foreign Windows cwd URI"));

    assert_eq!(
        apply_patch_cwd(&ctx).await.expect("execution cwd"),
        PathUri::parse("file:///C:/remote/project").unwrap()
    );
}

#[tokio::test]
async fn tool_spec_matches_codex_upstream_golden() {
    let tool = ApplyPatchTool::default();
    let tool: &dyn DynTool = &tool;
    let spec = tool
        .tool_spec(
            &coco_tool_runtime::SchemaContext::default(),
            &coco_tool_runtime::PromptOptions::default(),
        )
        .await;
    let coco_tool_runtime::ToolSpec::Freeform(spec) = spec else {
        panic!("apply_patch must be a Freeform tool, not Function");
    };
    let upstream = CODEX_APPLY_PATCH_SPEC_UPSTREAM_REVISION;
    assert_eq!(spec.name, "apply_patch", "Codex upstream: {upstream}");
    assert_eq!(
        spec.description, CODEX_APPLY_PATCH_DESCRIPTION_GOLDEN,
        "Codex upstream: {upstream}"
    );
    assert_eq!(spec.format.syntax, "lark", "Codex upstream: {upstream}");
    assert_eq!(
        spec.format.definition, CODEX_APPLY_PATCH_LARK_GRAMMAR_GOLDEN,
        "Codex upstream: {upstream}"
    );
}

#[test]
fn coerce_raw_string_input_wraps_patch() {
    let tool = ApplyPatchTool::default();
    let tool: &dyn DynTool = &tool;
    let raw = "*** Begin Patch\n*** Add File: a.txt\n+hi\n*** End Patch\n";
    let coerced = tool
        .coerce_raw_string_input(raw)
        .expect("apply_patch coerces a raw string");
    assert_eq!(coerced, serde_json::json!({ "patch": raw }));
    // The wrapped shape deserializes into the typed input.
    let input: ApplyPatchInput = serde_json::from_value(coerced).unwrap();
    assert_eq!(input.patch, raw);
}

#[test]
fn hook_projection_matches_codex_command_and_text_contract() {
    let tool = ApplyPatchTool::default();
    let runtime_input = serde_json::json!({ "patch": "*** Begin Patch\n*** End Patch" });

    assert_eq!(
        <ApplyPatchTool as Tool>::project_input_for_hooks(&tool, &runtime_input),
        serde_json::json!({ "command": "*** Begin Patch\n*** End Patch" })
    );
    assert_eq!(
        <ApplyPatchTool as Tool>::project_hook_input_to_runtime(
            &tool,
            serde_json::json!({ "command": "rewritten" }),
        )
        .expect("valid hook input"),
        serde_json::json!({ "patch": "rewritten" })
    );
    assert_eq!(
        <ApplyPatchTool as Tool>::project_output_for_hooks(
            &tool,
            &serde_json::json!({ "stdout": "Success.\n", "stderr": "warning\n" }),
        ),
        serde_json::json!("Success.\nwarning")
    );
}

#[test]
fn hook_projection_rejects_non_codex_updated_input() {
    let error = <ApplyPatchTool as Tool>::project_hook_input_to_runtime(
        &ApplyPatchTool::default(),
        serde_json::json!({ "patch": "legacy shape" }),
    )
    .expect_err("hook updates must use the Codex command field");

    assert!(error.contains("expected `command`"), "{error}");
}

#[test]
fn streamed_arguments_emit_structured_file_changes_without_duplicate_updates() {
    let tool = ApplyPatchTool::default();
    let mut consumer = <ApplyPatchTool as Tool>::argument_delta_consumer(&tool)
        .expect("apply_patch streaming consumer");

    let first = consumer
        .push_delta("*** Begin Patch\n*** Add File: new.txt\n")
        .expect("new hunk update");
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].path, "new.txt");
    assert_eq!(first[0].kind, coco_event_types::FileChangeKind::Create);
    assert!(consumer.push_delta("+content\n").is_none());

    let second = consumer
        .push_delta("*** Delete File: old.txt\n*** End Patch\n")
        .expect("second hunk update");
    assert_eq!(second.len(), 2);
    assert_eq!(second[1].path, "old.txt");
    assert_eq!(second[1].kind, coco_event_types::FileChangeKind::Delete);
    assert!(consumer.finish().is_none());
}

#[test]
fn apply_patch_is_never_tool_search_deferred() {
    // A Freeform tool has no JSON schema to defer and the `Provider` wire
    // variant can't carry `deferLoading` — apply_patch must always eager-load,
    // so it must never opt into ToolSearch deferral.
    let tool = ApplyPatchTool::default();
    let tool: &dyn DynTool = &tool;
    assert!(
        !tool.should_defer(),
        "apply_patch (Freeform) must never be ToolSearch-deferred"
    );
}

#[tokio::test]
async fn check_permissions_accept_edits_allows_cwd_patch() {
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(dir.path().to_path_buf());
    ctx.permission_context.mode = PermissionMode::AcceptEdits;
    let input = serde_json::json!({
        "patch": "*** Begin Patch\n*** Add File: notes.txt\n+hello\n*** End Patch\n"
    });

    let result =
        <ApplyPatchTool as DynTool>::check_permissions(&ApplyPatchTool::default(), &input, &ctx)
            .await;

    assert!(matches!(result, ToolCheckResult::Allow { .. }));
}

#[tokio::test]
async fn prepare_binds_paths_without_probing_patch_context() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("target.txt"), "secret\n").unwrap();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(dir.path().to_path_buf());
    let input = ApplyPatchInput {
        patch: "*** Begin Patch\n*** Update File: target.txt\n@@\n-guessed\n+replacement\n*** End Patch\n"
            .to_string(),
    };

    let prepared = <ApplyPatchTool as Tool>::prepare(&ApplyPatchTool::default(), &input, &ctx)
        .await
        .expect("path-only preparation must not test target contents");
    assert!(prepared.is_some());

    let error = <ApplyPatchTool as Tool>::execute_prepared(
        &ApplyPatchTool::default(),
        input,
        prepared,
        &ctx,
    )
    .await
    .expect_err("content mismatch is checked only in post-permission execution");
    assert!(error.to_string().contains("Failed to find expected lines"));
}

#[tokio::test]
async fn environment_header_is_rejected_instead_of_targeting_bound_file_system() {
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(dir.path().to_path_buf());
    let input = ApplyPatchInput {
        patch: "*** Begin Patch\n*** Environment ID: remote\n*** Add File: wrong.txt\n+wrong\n*** End Patch\n"
            .to_string(),
    };

    let error = <ApplyPatchTool as Tool>::prepare(&ApplyPatchTool::default(), &input, &ctx)
        .await
        .expect_err("unroutable environment selector must fail closed");

    assert!(
        error
            .to_string()
            .contains("environment selection is unavailable")
    );
    assert!(!dir.path().join("wrong.txt").exists());
}

#[tokio::test]
async fn invalid_duplicate_patch_bypasses_permission_ui_for_execution_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("duplicate.txt"), "before\n").unwrap();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(dir.path().to_path_buf());
    ctx.permission_context.mode = PermissionMode::Default;
    let input = serde_json::json!({
        "patch": "*** Begin Patch\n*** Update File: duplicate.txt\n@@\n-before\n+first\n*** Update File: ./duplicate.txt\n@@\n-before\n+second\n*** End Patch\n"
    });

    let result =
        <ApplyPatchTool as DynTool>::check_permissions(&ApplyPatchTool::default(), &input, &ctx)
            .await;

    assert!(matches!(result, ToolCheckResult::Allow { .. }));
}

#[cfg(unix)]
#[tokio::test]
async fn path_resolution_io_errors_do_not_bypass_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let invalid_cwd = dir.path().join("not-a-directory");
    std::fs::write(&invalid_cwd, "file\n").unwrap();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(invalid_cwd);
    ctx.permission_context.mode = PermissionMode::Default;
    let input = serde_json::json!({
        "patch": "*** Begin Patch\n*** Add File: child.txt\n+content\n*** End Patch\n"
    });

    let result =
        <ApplyPatchTool as DynTool>::check_permissions(&ApplyPatchTool::default(), &input, &ctx)
            .await;

    assert!(matches!(result, ToolCheckResult::Passthrough));
}

#[test]
fn apply_patch_result_bound_is_context_safe() {
    assert_eq!(
        <ApplyPatchTool as DynTool>::max_result_size_bound(&ApplyPatchTool::default()),
        coco_tool_runtime::ResultSizeBound::Bytes(3_000)
    );
}

#[tokio::test]
async fn check_permissions_path_scoped_edit_rule_allows_patch() {
    let dir = tempfile::Builder::new()
        .prefix("apply-patch-perms-")
        .tempdir_in(std::env::current_dir().unwrap())
        .unwrap();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(dir.path().to_path_buf());
    ctx.permission_context.allow_rules.insert(
        PermissionRuleSource::Session,
        vec![PermissionRule {
            source: PermissionRuleSource::Session,
            behavior: PermissionBehavior::Allow,
            value: PermissionRuleValue {
                tool_pattern: "Edit".into(),
                rule_content: Some(format!("/{}/**", dir.path().to_string_lossy())),
            },
        }],
    );
    let input = serde_json::json!({
        "patch": "*** Begin Patch\n*** Add File: notes.txt\n+hello\n*** End Patch\n"
    });

    let result =
        <ApplyPatchTool as DynTool>::check_permissions(&ApplyPatchTool::default(), &input, &ctx)
            .await;

    assert!(matches!(result, ToolCheckResult::Allow { .. }));
}

#[tokio::test]
async fn check_permissions_suspicious_path_requires_approval() {
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(dir.path().to_path_buf());
    ctx.permission_context.mode = PermissionMode::AcceptEdits;
    let input = serde_json::json!({
        "patch": "*** Begin Patch\n*** Add File: GIT~1/config\n+hello\n*** End Patch\n"
    });

    let result =
        <ApplyPatchTool as DynTool>::check_permissions(&ApplyPatchTool::default(), &input, &ctx)
            .await;

    assert!(matches!(result, ToolCheckResult::Ask { .. }));
}

#[tokio::test]
async fn check_permissions_mixed_internal_and_unsafe_paths_requires_approval() {
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(dir.path().to_path_buf());
    ctx.permission_context.mode = PermissionMode::AcceptEdits;
    let config_dir = coco_utils_common::COCO_CONFIG_DIR_NAME;
    let input = serde_json::json!({
        "patch": format!("*** Begin Patch\n*** Add File: {config_dir}/plans/plan.md\n+ok\n*** Add File: GIT~1/config\n+bad\n*** End Patch\n")
    });

    let result =
        <ApplyPatchTool as DynTool>::check_permissions(&ApplyPatchTool::default(), &input, &ctx)
            .await;

    assert!(matches!(result, ToolCheckResult::Ask { .. }));
}

#[tokio::test]
async fn check_permissions_default_ask_includes_write_suggestions() {
    let cwd = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(cwd.path().to_path_buf());
    ctx.permission_context.mode = PermissionMode::Default;
    let target = outside.path().join("notes.txt");
    let input = serde_json::json!({
        "patch": format!(
            "*** Begin Patch\n*** Add File: {}\n+hello\n*** End Patch\n",
            target.display()
        )
    });

    let result =
        <ApplyPatchTool as DynTool>::check_permissions(&ApplyPatchTool::default(), &input, &ctx)
            .await;

    let ToolCheckResult::Ask { suggestions, .. } = result else {
        panic!("expected ask");
    };
    assert!(suggestions.iter().any(|update| {
        matches!(
            update,
            coco_types::PermissionUpdate::SetMode {
                mode: PermissionMode::AcceptEdits
            }
        )
    }));
    let outside = outside.path().to_string_lossy().to_string();
    assert!(suggestions.iter().any(|update| {
        matches!(
            update,
            coco_types::PermissionUpdate::AddDirectories { directories, .. }
                if directories.iter().any(|dir| dir == &outside)
        )
    }));
}

#[tokio::test]
async fn check_permissions_move_to_disallowed_destination_is_denied() {
    let cwd = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(cwd.path().to_path_buf());
    ctx.allowed_write_roots = vec![cwd.path().to_path_buf()];
    ctx.permission_context.mode = PermissionMode::AcceptEdits;
    let destination = outside.path().join("renamed.rs");
    let input = serde_json::json!({
        "patch": format!(
            "*** Begin Patch\n*** Update File: source.rs\n*** Move to: {}\n@@\n-old\n+new\n*** End Patch\n",
            destination.display()
        )
    });

    let result =
        <ApplyPatchTool as DynTool>::check_permissions(&ApplyPatchTool::default(), &input, &ctx)
            .await;

    let ToolCheckResult::Deny { message } = result else {
        panic!("expected denied destination");
    };
    assert!(
        message.contains(&destination.display().to_string()),
        "{message}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn check_permissions_resolves_symlinked_parent_before_write_fence() {
    use std::os::unix::fs::symlink;

    let cwd = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    symlink(outside.path(), cwd.path().join("escape")).unwrap();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(cwd.path().to_path_buf());
    ctx.allowed_write_roots = vec![cwd.path().to_path_buf()];
    ctx.permission_context.mode = PermissionMode::AcceptEdits;
    let input = serde_json::json!({
        "patch": "*** Begin Patch\n*** Add File: escape/outside.txt\n+blocked\n*** End Patch\n"
    });

    let result =
        <ApplyPatchTool as DynTool>::check_permissions(&ApplyPatchTool::default(), &input, &ctx)
            .await;

    let ToolCheckResult::Deny { message } = result else {
        panic!("expected symlink escape to be denied, got {result:?}");
    };
    assert!(message.contains("resolves outside"), "{message}");
    assert!(
        !message.contains(&outside.path().display().to_string()),
        "canonical target topology leaked in denial: {message}"
    );
}

#[tokio::test]
async fn prepare_lexically_denies_outside_target_before_canonicalization() {
    let cwd = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let destination = outside.path().join("created.txt");
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(cwd.path().to_path_buf());
    ctx.allowed_write_roots = vec![cwd.path().to_path_buf()];
    let input = ApplyPatchInput {
        patch: format!(
            "*** Begin Patch\n*** Add File: {}\n+blocked\n*** End Patch\n",
            destination.display()
        ),
    };

    let prepared = <ApplyPatchTool as Tool>::prepare(&ApplyPatchTool::default(), &input, &ctx)
        .await
        .expect("lexical fence produces prepared denial state");
    let result = <ApplyPatchTool as Tool>::check_prepared_permissions(
        &ApplyPatchTool::default(),
        &input,
        prepared.as_ref(),
        &ctx,
    )
    .await;

    let ToolCheckResult::Deny { message } = result else {
        panic!("expected lexical fence denial, got {result:?}");
    };
    assert!(message.contains(&destination.display().to_string()));
}

#[tokio::test]
async fn check_permissions_allows_session_plan_file_in_sandboxed_agent() {
    // A sandboxed sub-agent (allowed_write_roots set) must still patch its own
    // session plan file (cocohome), which lives outside the worktree fence. The
    // internal-path exemption covers apply_patch's fence + permission checks.
    let cwd = tempfile::tempdir().unwrap();
    let plans = tempfile::tempdir().unwrap();
    let plan_file = plans.path().join("typed-conjuring-fox.md");
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(cwd.path().to_path_buf());
    ctx.allowed_write_roots = vec![cwd.path().to_path_buf()];
    ctx.permission_context.mode = PermissionMode::Plan;
    ctx.permission_context.bypass_available = false;
    ctx.permission_context.session_plan_file = Some(plan_file.clone());
    let input = serde_json::json!({
        "patch": format!(
            "*** Begin Patch\n*** Add File: {}\n+# plan\n*** End Patch\n",
            plan_file.display()
        )
    });

    let result =
        <ApplyPatchTool as DynTool>::check_permissions(&ApplyPatchTool::default(), &input, &ctx)
            .await;

    assert!(
        matches!(result, ToolCheckResult::Allow { .. }),
        "{result:?}"
    );
}

#[tokio::test]
async fn check_permissions_plan_file_carveout_fires_for_freeform_raw_string() {
    // calm-bouncing-biscuit regression: the custom-tool wire shape is a BARE
    // STRING. The agent loop coerces it through `ValidatedInput` before any
    // permission check — end-to-end here: raw string → coerce → dyn-level
    // check_permissions → plan-file auto-allow. (Pre-fix, the raw string hit
    // the blanket impl's deser failure, skipped this carve-out entirely, and
    // fell through to a Plan-mode prompt.)
    let cwd = tempfile::tempdir().unwrap();
    let plans = tempfile::tempdir().unwrap();
    let plan_file = plans.path().join("calm-bouncing-biscuit.md");
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(cwd.path().to_path_buf());
    ctx.permission_context.mode = PermissionMode::Plan;
    ctx.permission_context.bypass_available = false;
    ctx.permission_context.session_plan_file = Some(plan_file.clone());
    let raw = format!(
        "*** Begin Patch\n*** Add File: {}\n+# plan\n*** End Patch\n",
        plan_file.display()
    );

    let validated = coco_tool_runtime::ValidatedInput::validate(
        &ApplyPatchTool::default(),
        serde_json::Value::String(raw),
    )
    .expect("freeform raw string must coerce into {patch}");
    let result = <ApplyPatchTool as DynTool>::check_permissions(
        &ApplyPatchTool::default(),
        validated.as_value(),
        &ctx,
    )
    .await;

    assert!(
        matches!(result, ToolCheckResult::Allow { .. }),
        "plan-file write must auto-allow in Plan mode: {result:?}"
    );
}

#[test]
fn apply_patch_preview_add_file_uses_header_and_added_rows() {
    let preview = build_apply_patch_preview(
        "*** Begin Patch\n*** Add File: src/new.rs\n+fn main() {}\n+println!(\"hi\");\n*** End Patch",
    )
    .unwrap();

    assert_eq!(
        preview.rows,
        vec![
            coco_types::ApplyPatchPreviewRow::Header {
                action: coco_types::ApplyPatchPreviewAction::Add,
                target: "src/new.rs".to_string(),
            },
            coco_types::ApplyPatchPreviewRow::Line {
                sign: coco_types::ApplyPatchPreviewSign::Added,
                content: "fn main() {}".to_string(),
            },
            coco_types::ApplyPatchPreviewRow::Line {
                sign: coco_types::ApplyPatchPreviewSign::Added,
                content: "println!(\"hi\");".to_string(),
            },
        ]
    );
}

#[test]
fn apply_patch_preview_update_file_uses_signed_diff_rows() {
    let preview = build_apply_patch_preview(
        "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-old line\n+new line\n*** End Patch",
    )
    .unwrap();

    assert_eq!(
        preview.rows,
        vec![
            coco_types::ApplyPatchPreviewRow::Header {
                action: coco_types::ApplyPatchPreviewAction::Update,
                target: "src/lib.rs".to_string(),
            },
            coco_types::ApplyPatchPreviewRow::Line {
                sign: coco_types::ApplyPatchPreviewSign::Removed,
                content: "old line".to_string(),
            },
            coco_types::ApplyPatchPreviewRow::Line {
                sign: coco_types::ApplyPatchPreviewSign::Added,
                content: "new line".to_string(),
            },
        ]
    );
}

#[test]
fn apply_patch_preview_move_file_shows_source_and_destination() {
    let preview = build_apply_patch_preview(
        "*** Begin Patch\n*** Update File: old.rs\n*** Move to: new.rs\n@@\n-old_name()\n+new_name()\n*** End Patch",
    )
    .unwrap();

    assert_eq!(
        preview.rows[0],
        coco_types::ApplyPatchPreviewRow::Header {
            action: coco_types::ApplyPatchPreviewAction::Update,
            target: "old.rs -> new.rs".to_string(),
        }
    );
}

#[test]
fn apply_patch_preview_delete_file_uses_header_only() {
    let preview =
        build_apply_patch_preview("*** Begin Patch\n*** Delete File: obsolete.rs\n*** End Patch")
            .unwrap();

    assert_eq!(
        preview.rows,
        vec![coco_types::ApplyPatchPreviewRow::Header {
            action: coco_types::ApplyPatchPreviewAction::Delete,
            target: "obsolete.rs".to_string(),
        }]
    );
}

#[test]
fn apply_patch_preview_malformed_patch_falls_back_to_raw_rows() {
    let preview =
        build_apply_patch_preview("*** Update File: src/lib.rs\n-old line\n+new line\n").unwrap();

    assert_eq!(
        preview.rows,
        vec![
            coco_types::ApplyPatchPreviewRow::Raw {
                content: "*** Update File: src/lib.rs".to_string(),
            },
            coco_types::ApplyPatchPreviewRow::Line {
                sign: coco_types::ApplyPatchPreviewSign::Removed,
                content: "old line".to_string(),
            },
            coco_types::ApplyPatchPreviewRow::Line {
                sign: coco_types::ApplyPatchPreviewSign::Added,
                content: "new line".to_string(),
            },
        ]
    );
}

#[test]
fn apply_patch_preview_large_patch_keeps_head_and_tail() {
    let body = (0..260)
        .map(|i| format!("+line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let patch = format!("*** Begin Patch\n*** Add File: big.rs\n{body}\n*** End Patch");
    let preview = build_apply_patch_preview(&patch).unwrap();
    let text = serde_json::to_string(&preview.rows).unwrap();

    assert_eq!(preview.rows.len(), 200);
    assert!(
        preview
            .rows
            .contains(&coco_types::ApplyPatchPreviewRow::Omitted { rows: 62 })
    );
    assert!(text.contains("big.rs"), "{text}");
    assert!(text.contains("line 0"), "{text}");
    assert!(text.contains("line 259"), "{text}");
}

#[tokio::test]
async fn execute_result_includes_display_data_but_model_render_omits_it() {
    use coco_tool_runtime::ToolResultContentPart;
    use coco_types::ToolDisplayData;

    let dir = tempfile::tempdir().unwrap();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(dir.path().to_path_buf());
    let input = ApplyPatchInput {
        patch: "*** Begin Patch\n*** Add File: notes.txt\n+hello\n*** End Patch\n".to_string(),
    };

    let result = <ApplyPatchTool as Tool>::execute(&ApplyPatchTool::default(), input, &ctx)
        .await
        .unwrap();

    assert!(matches!(
        result.display_data,
        Some(ToolDisplayData::ApplyPatchPreview(_))
    ));
    let parts =
        <ApplyPatchTool as Tool>::render_for_model(&ApplyPatchTool::default(), &result.data);
    let [ToolResultContentPart::Text { text, .. }] = parts.as_slice() else {
        panic!("expected singleton text result");
    };
    assert!(!text.trim().is_empty());
    assert!(!text.contains("preview"), "{text}");
}

#[tokio::test]
async fn malformed_apply_patch_failure_keeps_display_data() {
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(dir.path().to_path_buf());
    let input = ApplyPatchInput {
        patch: "*** Update File: src/lib.rs\n-old line\n+new line\n".to_string(),
    };

    let err = <ApplyPatchTool as Tool>::execute(&ApplyPatchTool::default(), input, &ctx)
        .await
        .unwrap_err();

    let ToolError::ExecutionFailed {
        display_data: Some(display_data),
        ..
    } = err
    else {
        panic!("expected display-data execution failure");
    };
    let coco_types::ToolDisplayData::ApplyPatchPreview(preview) = display_data else {
        panic!("expected apply-patch preview display data");
    };
    assert_eq!(
        preview.rows[0],
        coco_types::ApplyPatchPreviewRow::Raw {
            content: "*** Update File: src/lib.rs".to_string(),
        }
    );
}

#[tokio::test]
async fn duplicate_resolved_paths_fail_before_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("duplicate.txt");
    std::fs::write(&target, "before\n").unwrap();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(dir.path().to_path_buf());
    let input = ApplyPatchInput {
        patch: "*** Begin Patch\n*** Update File: duplicate.txt\n@@\n-before\n+first\n*** Update File: ./duplicate.txt\n@@\n-before\n+second\n*** End Patch\n"
            .to_string(),
    };

    let error = <ApplyPatchTool as Tool>::execute(&ApplyPatchTool::default(), input, &ctx)
        .await
        .unwrap_err();

    let ToolError::ExecutionFailed {
        message,
        display_data: Some(_),
        ..
    } = error
    else {
        panic!("expected display-data execution failure");
    };
    assert!(message.contains("multiple operations target"), "{message}");
    assert_eq!(std::fs::read_to_string(target).unwrap(), "before\n");
}

#[tokio::test]
async fn execute_updates_multiple_distinct_paths() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first.txt");
    let second = dir.path().join("second.txt");
    std::fs::write(&first, "first before\n").unwrap();
    std::fs::write(&second, "second before\n").unwrap();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(dir.path().to_path_buf());
    let input = ApplyPatchInput {
        patch: "*** Begin Patch\n*** Update File: first.txt\n@@\n-first before\n+first after\n*** Update File: second.txt\n@@\n-second before\n+second after\n*** End Patch\n"
            .to_string(),
    };

    let result = <ApplyPatchTool as Tool>::execute(&ApplyPatchTool::default(), input, &ctx)
        .await
        .expect("apply distinct updates");

    assert_eq!(
        result.data.stdout,
        "Success. Updated the following files:\nM first.txt\nM second.txt\n"
    );
    assert_eq!(std::fs::read_to_string(&first).unwrap(), "first after\n");
    assert_eq!(std::fs::read_to_string(&second).unwrap(), "second after\n");
    let triggers = ctx.dynamic_skill_path_triggers.read().await;
    assert!(triggers.contains(&first.display().to_string()));
    assert!(triggers.contains(&second.display().to_string()));
}

#[tokio::test]
async fn execute_move_refreshes_file_state_for_destination_and_source() {
    use std::sync::Arc;

    use coco_context::FileReadEntry;
    use coco_context::FileReadState;
    use tokio::sync::RwLock;

    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.txt");
    let destination = dir.path().join("destination.txt");
    std::fs::write(&source, "before\n").unwrap();
    let source = std::fs::canonicalize(source).unwrap();
    let mtime = coco_utils_common::file_mtime_ms(&source).await.unwrap();
    let mut state = FileReadState::new();
    state.set(
        source.clone(),
        FileReadEntry::full_real("before\n".to_string(), mtime),
    );
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(dir.path().to_path_buf());
    ctx.file_read_state = Some(Arc::new(RwLock::new(state)));
    let input = ApplyPatchInput {
        patch: "*** Begin Patch\n*** Update File: source.txt\n*** Move to: destination.txt\n@@\n-before\n+after\n*** End Patch\n"
            .to_string(),
    };

    <ApplyPatchTool as Tool>::execute(&ApplyPatchTool::default(), input, &ctx)
        .await
        .expect("move file");

    let destination = std::fs::canonicalize(destination).unwrap();
    let state = ctx.file_read_state.as_ref().unwrap().read().await;
    assert!(state.peek(&source).is_none());
    assert_eq!(
        state.peek(&destination).map(|entry| entry.content.as_str()),
        Some("after\n")
    );
}

#[tokio::test]
async fn stale_commit_failure_invalidates_cache_without_triggering_skills() {
    use coco_context::FileReadEntry;
    use coco_context::FileReadState;
    use tokio::sync::RwLock;

    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("stale.txt");
    std::fs::write(&target, "external\n").unwrap();
    let target = std::fs::canonicalize(target).unwrap();
    let mtime = coco_utils_common::file_mtime_ms(&target).await.unwrap();
    let mut state = FileReadState::new();
    state.set(
        target.clone(),
        FileReadEntry::full_real("before\n".to_string(), mtime),
    );
    let mut ctx = ToolUseContext::test_default();
    ctx.file_read_state = Some(Arc::new(RwLock::new(state)));

    record_failed_commit(
        &ctx,
        &coco_apply_patch::AppliedPatchDelta::default(),
        &[coco_apply_patch::PreparedPatchPathOutcome {
            path: PathUri::from_path(&target).expect("target URI"),
            state: coco_apply_patch::PreparedPatchPathState::StaleExternal,
        }],
    )
    .await;

    assert!(
        ctx.file_read_state
            .as_ref()
            .unwrap()
            .read()
            .await
            .peek(&target)
            .is_none()
    );
    assert!(ctx.dynamic_skill_path_triggers.read().await.is_empty());
}

#[tokio::test]
async fn execute_preserves_crlf_line_endings() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("crlf.txt");
    std::fs::write(&target, b"before\r\n").unwrap();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(dir.path().to_path_buf());
    let input = ApplyPatchInput {
        patch: "*** Begin Patch\n*** Update File: crlf.txt\n@@\n-before\n+after\n*** End Patch\n"
            .to_string(),
    };

    <ApplyPatchTool as Tool>::execute(&ApplyPatchTool::default(), input, &ctx)
        .await
        .expect("apply CRLF update");

    assert_eq!(std::fs::read(target).unwrap(), b"after\r\n");
}

#[tokio::test]
async fn execute_denies_move_destination_outside_allowed_write_roots() {
    let cwd = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let source = cwd.path().join("source.rs");
    std::fs::write(&source, "old\n").unwrap();
    let destination = outside.path().join("renamed.rs");
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(cwd.path().to_path_buf());
    ctx.allowed_write_roots = vec![cwd.path().to_path_buf()];
    let input = ApplyPatchInput {
        patch: format!(
            "*** Begin Patch\n*** Update File: source.rs\n*** Move to: {}\n@@\n-old\n+new\n*** End Patch\n",
            destination.display()
        ),
    };

    let err = <ApplyPatchTool as Tool>::execute(&ApplyPatchTool::default(), input, &ctx)
        .await
        .unwrap_err();

    let ToolError::ExecutionFailed {
        message,
        display_data: Some(_),
        ..
    } = err
    else {
        panic!("expected display-data execution failure");
    };
    assert!(
        message.contains(&destination.display().to_string()),
        "{message}"
    );
    assert!(source.exists());
    assert!(!destination.exists());
}

#[tokio::test]
async fn execute_rejects_secret_add_to_team_memory_path() {
    let dir = tempfile::tempdir().unwrap();
    let team_dir = dir
        .path()
        .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
        .join("memory")
        .join("team");
    std::fs::create_dir_all(&team_dir).unwrap();
    let target = team_dir.join("token.md");
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(dir.path().to_path_buf());
    let config_dir = coco_utils_common::COCO_CONFIG_DIR_NAME;
    let input = ApplyPatchInput {
        patch: format!(
            "*** Begin Patch\n*** Add File: {config_dir}/memory/team/token.md\n+API_KEY=sk-ant-AAAAAAAAAAAAAAAAAAAAAA\n*** End Patch\n"
        ),
    };

    let err = <ApplyPatchTool as Tool>::execute(&ApplyPatchTool::default(), input, &ctx)
        .await
        .unwrap_err();

    let ToolError::ExecutionFailed {
        message,
        display_data: Some(_),
        ..
    } = err
    else {
        panic!("expected display-data execution failure");
    };
    assert!(message.contains("secret"), "{message}");
    assert!(!target.exists());
}

#[tokio::test]
async fn execute_rejects_secret_update_to_team_memory_path() {
    let dir = tempfile::tempdir().unwrap();
    let team_dir = dir
        .path()
        .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
        .join("memory")
        .join("team");
    std::fs::create_dir_all(&team_dir).unwrap();
    let target = team_dir.join("token.md");
    std::fs::write(&target, "API_KEY=placeholder\n").unwrap();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(dir.path().to_path_buf());
    let config_dir = coco_utils_common::COCO_CONFIG_DIR_NAME;
    let input = ApplyPatchInput {
        patch: format!(
            "*** Begin Patch\n*** Update File: {config_dir}/memory/team/token.md\n@@\n-API_KEY=placeholder\n+API_KEY=sk-ant-AAAAAAAAAAAAAAAAAAAAAA\n*** End Patch\n"
        ),
    };

    let err = <ApplyPatchTool as Tool>::execute(&ApplyPatchTool::default(), input, &ctx)
        .await
        .unwrap_err();

    let ToolError::ExecutionFailed {
        message,
        display_data: Some(_),
        ..
    } = err
    else {
        panic!("expected display-data execution failure");
    };
    assert!(message.contains("secret"), "{message}");
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "API_KEY=placeholder\n"
    );
}

#[test]
fn apply_patch_preview_caps_large_row_content() {
    let long = "x".repeat(APPLY_PATCH_PREVIEW_ROW_CHARS + 50);
    let patch = format!("*** Begin Patch\n*** Add File: big.rs\n+{long}\n*** End Patch");
    let preview = build_apply_patch_preview(&patch).unwrap();

    let Some(coco_types::ApplyPatchPreviewRow::Line { content, .. }) = preview.rows.get(1) else {
        panic!("expected content row");
    };
    assert_eq!(content.chars().count(), APPLY_PATCH_PREVIEW_ROW_CHARS);
}
