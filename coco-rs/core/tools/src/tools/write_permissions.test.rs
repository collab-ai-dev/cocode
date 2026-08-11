use super::*;

fn context_for(cwd: &Path, mode: PermissionMode) -> ToolUseContext {
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(cwd.to_path_buf());
    ctx.permission_context.mode = mode;
    ctx
}

#[test]
fn protected_instruction_files_always_require_operation_approval() {
    let temp = tempfile::tempdir().unwrap();

    for mode in [
        PermissionMode::AcceptEdits,
        PermissionMode::BypassPermissions,
    ] {
        for name in [
            "CLAUDE.md",
            "agents.md",
            "CLAUDE.local.md",
            "AGENTS.local.md",
        ] {
            let ctx = context_for(temp.path(), mode);
            let result = check_write_permission_for_path(
                temp.path().join(name).to_str().unwrap(),
                &ctx,
                ToolName::Write.as_str(),
                "write to a file",
            );
            let ToolCheckResult::Ask { suggestions, .. } = result else {
                panic!("{name} should require approval in {mode:?}");
            };
            assert!(suggestions.is_empty());
        }
    }
}

#[cfg(unix)]
#[test]
fn canonical_target_prevents_symlink_bypass() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("AGENTS.md");
    std::fs::write(&target, "instructions").unwrap();
    let link = temp.path().join("ordinary.md");

    std::os::unix::fs::symlink(&target, &link).unwrap();

    let ctx = context_for(temp.path(), PermissionMode::BypassPermissions);
    let result = check_write_permission_for_path(
        link.to_str().unwrap(),
        &ctx,
        ToolName::Edit.as_str(),
        "edit a file",
    );

    assert!(matches!(result, ToolCheckResult::Ask { .. }));
}

#[test]
fn normal_files_keep_existing_accept_edits_behavior() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = context_for(temp.path(), PermissionMode::AcceptEdits);

    let result = check_write_permission_for_path(
        temp.path().join("src.rs").to_str().unwrap(),
        &ctx,
        ToolName::Write.as_str(),
        "write to a file",
    );

    assert!(matches!(result, ToolCheckResult::Allow { .. }));
}
