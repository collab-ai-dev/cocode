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

#[test]
fn is_enabled_only_when_model_adds_apply_patch() {
    let tool: &dyn DynTool = &ApplyPatchTool;

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
}

#[tokio::test]
async fn tool_spec_is_freeform_lark_grammar() {
    let tool: &dyn DynTool = &ApplyPatchTool;
    let spec = tool
        .tool_spec(
            &coco_tool_runtime::SchemaContext::default(),
            &coco_tool_runtime::PromptOptions::default(),
        )
        .await;
    let coco_tool_runtime::ToolSpec::Freeform(spec) = spec else {
        panic!("apply_patch must be a Freeform tool, not Function");
    };
    assert_eq!(spec.name, ToolName::ApplyPatch.as_str());
    assert_eq!(spec.format.syntax, "lark");
    // The grammar is the codex envelope (begin/end + EOF marker).
    assert!(spec.format.definition.contains("*** Begin Patch"));
    assert!(spec.format.definition.contains("*** End of File"));
    assert!(!spec.description.trim().is_empty());
}

#[test]
fn coerce_raw_string_input_wraps_patch() {
    let tool: &dyn DynTool = &ApplyPatchTool;
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
fn apply_patch_is_never_tool_search_deferred() {
    // A Freeform tool has no JSON schema to defer and the `Provider` wire
    // variant can't carry `deferLoading` — apply_patch must always eager-load,
    // so it must never opt into ToolSearch deferral.
    let tool: &dyn DynTool = &ApplyPatchTool;
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
        <ApplyPatchTool as DynTool>::check_permissions(&ApplyPatchTool, &input, &ctx).await;

    assert!(matches!(result, ToolCheckResult::Allow { .. }));
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
        <ApplyPatchTool as DynTool>::check_permissions(&ApplyPatchTool, &input, &ctx).await;

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
        <ApplyPatchTool as DynTool>::check_permissions(&ApplyPatchTool, &input, &ctx).await;

    assert!(matches!(result, ToolCheckResult::Ask { .. }));
}

#[tokio::test]
async fn check_permissions_mixed_internal_and_unsafe_paths_requires_approval() {
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(dir.path().to_path_buf());
    ctx.permission_context.mode = PermissionMode::AcceptEdits;
    let input = serde_json::json!({
        "patch": "*** Begin Patch\n*** Add File: .coco/plans/plan.md\n+ok\n*** Add File: GIT~1/config\n+bad\n*** End Patch\n"
    });

    let result =
        <ApplyPatchTool as DynTool>::check_permissions(&ApplyPatchTool, &input, &ctx).await;

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
        <ApplyPatchTool as DynTool>::check_permissions(&ApplyPatchTool, &input, &ctx).await;

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
        <ApplyPatchTool as DynTool>::check_permissions(&ApplyPatchTool, &input, &ctx).await;

    let ToolCheckResult::Deny { message } = result else {
        panic!("expected denied destination");
    };
    assert!(
        message.contains(&destination.display().to_string()),
        "{message}"
    );
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
        <ApplyPatchTool as DynTool>::check_permissions(&ApplyPatchTool, &input, &ctx).await;

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
        &ApplyPatchTool,
        serde_json::Value::String(raw),
    )
    .expect("freeform raw string must coerce into {patch}");
    let result =
        <ApplyPatchTool as DynTool>::check_permissions(&ApplyPatchTool, validated.as_value(), &ctx)
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

    let result = <ApplyPatchTool as Tool>::execute(&ApplyPatchTool, input, &ctx)
        .await
        .unwrap();

    assert!(matches!(
        result.display_data,
        Some(ToolDisplayData::ApplyPatchPreview(_))
    ));
    let parts = <ApplyPatchTool as Tool>::render_for_model(&ApplyPatchTool, &result.data);
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

    let err = <ApplyPatchTool as Tool>::execute(&ApplyPatchTool, input, &ctx)
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

    let err = <ApplyPatchTool as Tool>::execute(&ApplyPatchTool, input, &ctx)
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
    let team_dir = dir.path().join(".coco").join("memory").join("team");
    std::fs::create_dir_all(&team_dir).unwrap();
    let target = team_dir.join("token.md");
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(dir.path().to_path_buf());
    let input = ApplyPatchInput {
        patch: "*** Begin Patch\n*** Add File: .coco/memory/team/token.md\n+API_KEY=sk-ant-AAAAAAAAAAAAAAAAAAAAAA\n*** End Patch\n".to_string(),
    };

    let err = <ApplyPatchTool as Tool>::execute(&ApplyPatchTool, input, &ctx)
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
    let team_dir = dir.path().join(".coco").join("memory").join("team");
    std::fs::create_dir_all(&team_dir).unwrap();
    let target = team_dir.join("token.md");
    std::fs::write(&target, "API_KEY=placeholder\n").unwrap();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(dir.path().to_path_buf());
    let input = ApplyPatchInput {
        patch: "*** Begin Patch\n*** Update File: .coco/memory/team/token.md\n@@\n-API_KEY=placeholder\n+API_KEY=sk-ant-AAAAAAAAAAAAAAAAAAAAAA\n*** End Patch\n".to_string(),
    };

    let err = <ApplyPatchTool as Tool>::execute(&ApplyPatchTool, input, &ctx)
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
