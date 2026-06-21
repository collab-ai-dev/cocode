//! Shared write permission checks for file-editing tools.

use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

use coco_tool_runtime::ToolUseContext;
use coco_types::PermissionMode;
use coco_types::PermissionRuleSource;
use coco_types::PermissionUpdate;
use coco_types::PermissionUpdateDestination;
use coco_types::ToolCheckResult;
use coco_types::ToolName;

pub(crate) fn check_write_permission_for_path(
    path: &str,
    ctx: &ToolUseContext,
    tool_name: &str,
    action: &str,
) -> ToolCheckResult {
    let cwd = effective_cwd(ctx);
    let cwd_str = cwd.to_string_lossy();
    let paths_to_check =
        coco_permissions::filesystem::get_paths_for_permission_check(path, &cwd_str);
    check_write_permission_for_paths(&paths_to_check, ctx, tool_name, action, &cwd)
}

pub(crate) fn check_write_permission_for_paths(
    paths_to_check: &[String],
    ctx: &ToolUseContext,
    tool_name: &str,
    action: &str,
    cwd: &Path,
) -> ToolCheckResult {
    if crate::tools::read_permissions::matching_edit_rule(
        &ctx.permission_context.deny_rules,
        paths_to_check,
        cwd,
        &ctx.permission_context,
    )
    .is_some()
    {
        return ToolCheckResult::Deny {
            message: format!("Permission to {action} has been denied by an edit rule."),
        };
    }

    let cwd_str = cwd.to_string_lossy();
    let internal_ctx = coco_permissions::filesystem::InternalPathContext {
        cwd: &cwd_str,
        session_plan_file: ctx.permission_context.session_plan_file.as_deref(),
    };
    if !paths_to_check.is_empty()
        && paths_to_check.iter().all(|path| {
            coco_permissions::filesystem::is_editable_internal_path(path, &internal_ctx)
        })
    {
        return ToolCheckResult::Allow {
            updated_input: None,
            feedback: None,
        };
    }

    for path in paths_to_check {
        if let coco_permissions::filesystem::PathSafetyResult::Blocked { message, .. } =
            coco_permissions::filesystem::check_path_safety_for_auto_edit(path)
        {
            return ToolCheckResult::Ask {
                message,
                suggestions: write_permission_suggestions(paths_to_check, &cwd_str, ctx),
                choices: None,
                detail: None,
            };
        }
    }

    if crate::tools::read_permissions::matching_edit_rule(
        &ctx.permission_context.ask_rules,
        paths_to_check,
        cwd,
        &ctx.permission_context,
    )
    .is_some()
    {
        return ToolCheckResult::Ask {
            message: format!(
                "Coco requested permissions to {action}, but you haven't granted it yet."
            ),
            suggestions: vec![],
            choices: None,
            detail: None,
        };
    }

    if ctx.permission_context.mode == PermissionMode::AcceptEdits
        && paths_to_check
            .iter()
            .all(|path| path_is_in_working_dirs(path, &cwd_str, ctx))
    {
        return ToolCheckResult::Allow {
            updated_input: None,
            feedback: None,
        };
    }

    if all_paths_have_matching_edit_allow(paths_to_check, ctx, cwd) {
        return ToolCheckResult::Allow {
            updated_input: None,
            feedback: None,
        };
    }

    if tool_wide_allow_rule_exists(&ctx.permission_context.allow_rules, tool_name) {
        return ToolCheckResult::Allow {
            updated_input: None,
            feedback: None,
        };
    }

    match ctx.permission_context.mode {
        PermissionMode::Default
        | PermissionMode::Auto
        | PermissionMode::Bubble
        | PermissionMode::AcceptEdits => ToolCheckResult::Ask {
            message: format!(
                "Coco requested permissions to {action}, but you haven't granted it yet."
            ),
            suggestions: write_permission_suggestions(paths_to_check, &cwd_str, ctx),
            choices: None,
            detail: None,
        },
        PermissionMode::Plan if !ctx.permission_context.bypass_available => ToolCheckResult::Ask {
            message: format!(
                "Coco requested permissions to {action}, but you haven't granted it yet."
            ),
            suggestions: write_permission_suggestions(paths_to_check, &cwd_str, ctx),
            choices: None,
            detail: None,
        },
        PermissionMode::Plan | PermissionMode::BypassPermissions | PermissionMode::DontAsk => {
            ToolCheckResult::Passthrough
        }
    }
}

pub(crate) fn effective_cwd(ctx: &ToolUseContext) -> PathBuf {
    ctx.cwd_override
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub(crate) fn write_permission_suggestions(
    paths_to_check: &[String],
    cwd: &str,
    ctx: &ToolUseContext,
) -> Vec<PermissionUpdate> {
    let mut updates = Vec::new();
    if matches!(
        ctx.permission_context.mode,
        PermissionMode::Default | PermissionMode::Plan
    ) {
        updates.push(PermissionUpdate::SetMode {
            mode: PermissionMode::AcceptEdits,
        });
    }

    let mut dirs = BTreeSet::new();
    for path in paths_to_check {
        if path_is_in_working_dirs(path, cwd, ctx) {
            continue;
        }
        if let Some(dir) = directory_for_path(path, cwd) {
            for dir_to_add in
                coco_permissions::filesystem::get_paths_for_permission_check(&dir, cwd)
            {
                dirs.insert(dir_to_add);
            }
        }
    }
    if !dirs.is_empty() {
        updates.push(PermissionUpdate::AddDirectories {
            directories: dirs.into_iter().collect(),
            destination: PermissionUpdateDestination::Session,
        });
    }
    updates
}

fn directory_for_path(path: &str, cwd: &str) -> Option<String> {
    let raw = PathBuf::from(path);
    let absolute = if raw.is_absolute() {
        raw
    } else {
        PathBuf::from(cwd).join(raw)
    };
    let dir = if absolute.is_dir() {
        absolute
    } else {
        absolute.parent()?.to_path_buf()
    };
    Some(dir.to_string_lossy().to_string())
}

fn path_is_in_working_dirs(path: &str, cwd: &str, ctx: &ToolUseContext) -> bool {
    if coco_permissions::filesystem::path_in_working_path(path, cwd) {
        return true;
    }
    ctx.permission_context
        .additional_dirs
        .values()
        .any(|dir| coco_permissions::filesystem::path_in_working_path(path, &dir.path))
}

fn all_paths_have_matching_edit_allow(
    paths_to_check: &[String],
    ctx: &ToolUseContext,
    cwd: &Path,
) -> bool {
    paths_to_check.iter().all(|path| {
        crate::tools::read_permissions::matching_edit_rule(
            &ctx.permission_context.allow_rules,
            std::slice::from_ref(path),
            cwd,
            &ctx.permission_context,
        )
        .is_some()
    })
}

fn tool_wide_allow_rule_exists(
    rules: &coco_types::PermissionRulesBySource,
    tool_name: &str,
) -> bool {
    const SOURCE_ORDER: &[PermissionRuleSource] = &[
        PermissionRuleSource::Session,
        PermissionRuleSource::Command,
        PermissionRuleSource::CliArg,
        PermissionRuleSource::FlagSettings,
        PermissionRuleSource::LocalSettings,
        PermissionRuleSource::ProjectSettings,
        PermissionRuleSource::UserSettings,
        PermissionRuleSource::PolicySettings,
    ];

    SOURCE_ORDER.iter().any(|source| {
        rules.get(source).is_some_and(|source_rules| {
            source_rules.iter().any(|rule| {
                rule.value.rule_content.is_none()
                    && (rule.value.tool_pattern == tool_name
                        || (tool_name == ToolName::ApplyPatch.as_str()
                            && rule.value.tool_pattern == ToolName::Edit.as_str()))
            })
        })
    })
}
