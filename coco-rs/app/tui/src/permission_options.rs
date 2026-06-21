//! Shared classic permission-option model.
//!
//! Rendering and resolution both use this module so the rows shown to the
//! user are exactly the actions that can produce meaningful updates.

use std::str::FromStr;

use crate::state::PermissionPromptState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PermissionAction {
    ApproveOnce,
    AllowSession,
    AllowLocal,
    Deny,
}

pub(crate) fn classic_actions(
    p: &PermissionPromptState,
    current_mode: coco_types::PermissionMode,
) -> Vec<PermissionAction> {
    let mut actions = vec![PermissionAction::ApproveOnce];
    if p.show_always_allow {
        if p.prefix_input.is_some() {
            // Shell tools with an editable prefix always offer both allow rows
            // so the row set stays stable while the user edits the field (an
            // emptied field commits as allow-once). Gating on non-empty updates
            // would make a row vanish mid-edit and desync `selected_choice`.
            actions.push(PermissionAction::AllowSession);
            actions.push(PermissionAction::AllowLocal);
        } else {
            if !session_allow_updates(p, current_mode).is_empty() {
                actions.push(PermissionAction::AllowSession);
            }
            if !local_allow_updates(p).is_empty() {
                actions.push(PermissionAction::AllowLocal);
            }
        }
    }
    actions.push(PermissionAction::Deny);
    actions
}

pub(crate) fn classic_action_at(
    p: &PermissionPromptState,
    current_mode: coco_types::PermissionMode,
    index: usize,
) -> PermissionAction {
    classic_actions(p, current_mode)
        .get(index)
        .copied()
        .unwrap_or(PermissionAction::Deny)
}

pub(crate) fn selected_classic_action(
    p: &PermissionPromptState,
    current_mode: coco_types::PermissionMode,
) -> PermissionAction {
    let actions = classic_actions(p, current_mode);
    let index = p.selected_choice.min(actions.len().saturating_sub(1));
    actions
        .get(index)
        .copied()
        .unwrap_or(PermissionAction::Deny)
}

/// Whether the editable "always allow" prefix field is active for input: the
/// prompt carries a `prefix_input` (shell tool) and the focused classic row is
/// an always-allow row. When true, typed characters edit the prefix rather than
/// triggering y/n/a hotkeys (the `PermissionPrefixEdit` keybinding context).
pub(crate) fn prefix_editing(
    p: &PermissionPromptState,
    current_mode: coco_types::PermissionMode,
) -> bool {
    p.prefix_input.is_some()
        && matches!(
            selected_classic_action(p, current_mode),
            PermissionAction::AllowSession | PermissionAction::AllowLocal
        )
}

/// The shell allow rule built from the edited prefix, if the prompt has a
/// non-empty editable prefix that parses to a safe (Exact/Prefix) shell rule.
/// `None` → no edited prefix in play; the caller falls back to the engine
/// suggestion. An empty edited prefix also yields `None` (empty → allow
/// once, no rule).
fn edited_prefix_rule(
    p: &PermissionPromptState,
    source: coco_types::PermissionRuleSource,
) -> Option<coco_types::PermissionRule> {
    let value = p.prefix_input.as_ref()?.value.trim();
    if value.is_empty() {
        return None;
    }
    let rule = coco_types::PermissionRule {
        source,
        behavior: coco_types::PermissionBehavior::Allow,
        value: coco_types::PermissionRuleValue {
            tool_pattern: p.tool_name.clone(),
            rule_content: Some(value.to_string()),
        },
    };
    scoped_allow_rule_is_safe(&rule).then_some(rule)
}

pub(crate) fn session_allow_updates(
    p: &PermissionPromptState,
    current_mode: coco_types::PermissionMode,
) -> Vec<coco_types::PermissionUpdate> {
    // Shell tools with an editable prefix: the edited value is authoritative
    // (replaces the engine suggestion). An empty / unsafe value yields no
    // rule → commit allows once.
    if p.prefix_input.is_some() {
        return edited_prefix_rule(p, coco_types::PermissionRuleSource::Session)
            .map(|rule| {
                vec![coco_types::PermissionUpdate::AddRules {
                    rules: vec![rule],
                    destination: coco_types::PermissionUpdateDestination::Session,
                }]
            })
            .unwrap_or_default();
    }
    let mut updates = Vec::new();
    let mut rules = Vec::new();
    for suggestion in &p.permission_suggestions {
        match suggestion {
            coco_types::PermissionUpdate::SetMode {
                mode: coco_types::PermissionMode::AcceptEdits,
            } if matches!(
                current_mode,
                coco_types::PermissionMode::Default | coco_types::PermissionMode::Plan
            ) =>
            {
                updates.push(suggestion.clone());
            }
            coco_types::PermissionUpdate::AddRules {
                rules: suggested, ..
            } => {
                rules.extend(scoped_allow_rules(
                    suggested,
                    coco_types::PermissionRuleSource::Session,
                ));
            }
            coco_types::PermissionUpdate::AddDirectories { directories, .. }
                if !directories.is_empty() =>
            {
                updates.push(coco_types::PermissionUpdate::AddDirectories {
                    directories: directories.clone(),
                    destination: coco_types::PermissionUpdateDestination::Session,
                });
            }
            coco_types::PermissionUpdate::SetMode { .. }
            | coco_types::PermissionUpdate::ReplaceRules { .. }
            | coco_types::PermissionUpdate::RemoveRules { .. }
            | coco_types::PermissionUpdate::AddDirectories { .. }
            | coco_types::PermissionUpdate::RemoveDirectories { .. } => {}
        }
    }
    if !rules.is_empty() {
        updates.push(coco_types::PermissionUpdate::AddRules {
            rules,
            destination: coco_types::PermissionUpdateDestination::Session,
        });
    }
    if updates.is_empty()
        && let Some(update) = read_path_allow_update(
            &p.tool_name,
            p.original_input.as_ref(),
            p.cwd.as_deref(),
            coco_types::PermissionRuleSource::Session,
            coco_types::PermissionUpdateDestination::Session,
        )
    {
        updates.push(update);
    }
    // No suggestion and no derivable path: fall back to an exact-tool-name
    // session allow (MCP tools etc.). Dangerous tool-wide grants for
    // file/shell tools are filtered out by `tool_wide_allow_update`.
    if updates.is_empty()
        && let Some(update) = tool_wide_allow_update(
            &p.tool_name,
            coco_types::PermissionRuleSource::Session,
            coco_types::PermissionUpdateDestination::Session,
        )
    {
        updates.push(update);
    }
    updates
}

pub(crate) fn local_allow_updates(p: &PermissionPromptState) -> Vec<coco_types::PermissionUpdate> {
    // Shell editable prefix is authoritative (see `session_allow_updates`).
    if p.prefix_input.is_some() {
        return edited_prefix_rule(p, coco_types::PermissionRuleSource::LocalSettings)
            .map(|rule| {
                vec![coco_types::PermissionUpdate::AddRules {
                    rules: vec![rule],
                    destination: coco_types::PermissionUpdateDestination::LocalSettings,
                }]
            })
            .unwrap_or_default();
    }
    // Directory-boundary ask (out-of-dir bash write/redirect): mirror the
    // engine's `AddDirectories` suggestion to LocalSettings so "always allow"
    // persists the working-dir grant — the only update that unblocks the path
    // gate. `session_allow_updates` already maps the same suggestion to Session.
    let dir_updates: Vec<_> = p
        .permission_suggestions
        .iter()
        .filter_map(|suggestion| match suggestion {
            coco_types::PermissionUpdate::AddDirectories { directories, .. }
                if !directories.is_empty() =>
            {
                Some(coco_types::PermissionUpdate::AddDirectories {
                    directories: directories.clone(),
                    destination: coco_types::PermissionUpdateDestination::LocalSettings,
                })
            }
            coco_types::PermissionUpdate::AddDirectories { .. }
            | coco_types::PermissionUpdate::AddRules { .. }
            | coco_types::PermissionUpdate::SetMode { .. }
            | coco_types::PermissionUpdate::ReplaceRules { .. }
            | coco_types::PermissionUpdate::RemoveRules { .. }
            | coco_types::PermissionUpdate::RemoveDirectories { .. } => None,
        })
        .collect();
    if !dir_updates.is_empty() {
        return dir_updates;
    }
    if let Some(update) = read_path_allow_update(
        &p.tool_name,
        p.original_input.as_ref(),
        p.cwd.as_deref(),
        coco_types::PermissionRuleSource::LocalSettings,
        coco_types::PermissionUpdateDestination::LocalSettings,
    ) {
        return vec![update];
    }
    if let Some(update) = edit_path_allow_update(
        &p.tool_name,
        p.original_input.as_ref(),
        p.cwd.as_deref(),
        coco_types::PermissionRuleSource::LocalSettings,
        coco_types::PermissionUpdateDestination::LocalSettings,
    ) {
        return vec![update];
    }

    let rules = p
        .permission_suggestions
        .iter()
        .filter_map(|suggestion| match suggestion {
            coco_types::PermissionUpdate::AddRules { rules, .. } => Some(rules.as_slice()),
            coco_types::PermissionUpdate::SetMode { .. }
            | coco_types::PermissionUpdate::ReplaceRules { .. }
            | coco_types::PermissionUpdate::RemoveRules { .. }
            | coco_types::PermissionUpdate::AddDirectories { .. }
            | coco_types::PermissionUpdate::RemoveDirectories { .. } => None,
        })
        .flat_map(|rules| {
            scoped_allow_rules(rules, coco_types::PermissionRuleSource::LocalSettings)
        })
        .collect::<Vec<_>>();
    if rules.is_empty() {
        // No scoped suggestion and no derivable path: persist an exact-
        // tool-name allow (MCP tools etc.); filtered out for file/shell tools.
        tool_wide_allow_update(
            &p.tool_name,
            coco_types::PermissionRuleSource::LocalSettings,
            coco_types::PermissionUpdateDestination::LocalSettings,
        )
        .into_iter()
        .collect()
    } else {
        vec![coco_types::PermissionUpdate::AddRules {
            rules,
            destination: coco_types::PermissionUpdateDestination::LocalSettings,
        }]
    }
}

fn scoped_allow_rules(
    rules: &[coco_types::PermissionRule],
    source: coco_types::PermissionRuleSource,
) -> Vec<coco_types::PermissionRule> {
    rules
        .iter()
        .filter(|rule| scoped_allow_rule_is_safe(rule))
        .map(|rule| coco_types::PermissionRule {
            source,
            behavior: coco_types::PermissionBehavior::Allow,
            value: rule.value.clone(),
        })
        .collect()
}

fn scoped_allow_rule_is_safe(rule: &coco_types::PermissionRule) -> bool {
    if rule.behavior != coco_types::PermissionBehavior::Allow {
        return false;
    }
    let tool_pattern = rule.value.tool_pattern.trim();
    if tool_pattern.is_empty() || tool_pattern == "*" {
        return false;
    }
    let Some(rule_content) = rule.value.rule_content.as_deref() else {
        return false;
    };
    if rule_content.is_empty() {
        return false;
    }

    if tool_pattern == coco_types::ToolName::Bash.as_str()
        || tool_pattern == coco_types::ToolName::PowerShell.as_str()
    {
        return matches!(
            coco_permissions::ShellPermissionRule::parse(rule_content),
            coco_permissions::ShellPermissionRule::Exact { .. }
                | coco_permissions::ShellPermissionRule::Prefix { .. }
        );
    }

    true
}

/// Tools that carry a path- or command-scoping concept: a TOOL-WIDE allow
/// (no `rule_content`) would over-grant ("read/edit/run anything"), so these
/// are scoped-to-a-directory-or-command or not offered at all — never widened
/// to tool-wide when the path can't be derived (fail-closed). Every other tool
/// (MCP `mcp__*`, …) has no narrower scope, so it falls back to an exact-tool-
/// name allow.
fn tool_requires_scoped_allow(tool_name: &str) -> bool {
    matches!(
        coco_types::ToolName::from_str(tool_name),
        Ok(coco_types::ToolName::Read
            | coco_types::ToolName::Grep
            | coco_types::ToolName::Glob
            | coco_types::ToolName::Edit
            | coco_types::ToolName::Write
            | coco_types::ToolName::NotebookEdit
            | coco_types::ToolName::ApplyPatch
            | coco_types::ToolName::Bash
            | coco_types::ToolName::PowerShell)
    )
}

/// Exact-tool-name allow rule (`mcp__server__tool`, `WebFetch`, …) for tools
/// that produced no scoped suggestion. Returns `None` for the dangerous
/// tool-wide set and for empty / wildcard tool names.
fn tool_wide_allow_update(
    tool_name: &str,
    source: coco_types::PermissionRuleSource,
    destination: coco_types::PermissionUpdateDestination,
) -> Option<coco_types::PermissionUpdate> {
    let tool_name = tool_name.trim();
    if tool_name.is_empty() || tool_name == "*" || tool_requires_scoped_allow(tool_name) {
        return None;
    }
    Some(coco_types::PermissionUpdate::AddRules {
        rules: vec![coco_types::PermissionRule {
            source,
            behavior: coco_types::PermissionBehavior::Allow,
            value: coco_types::PermissionRuleValue {
                tool_pattern: tool_name.to_string(),
                rule_content: None,
            },
        }],
        destination,
    })
}

/// Directory-scoped `Edit(dir/**)` allow rule for write-capable tools.
pub(crate) fn edit_path_allow_update(
    tool_name: &str,
    original_input: Option<&serde_json::Value>,
    cwd: Option<&str>,
    source: coco_types::PermissionRuleSource,
    destination: coco_types::PermissionUpdateDestination,
) -> Option<coco_types::PermissionUpdate> {
    let tool = coco_types::ToolName::from_str(tool_name).ok()?;
    let input = original_input?;
    let paths: Vec<&str> = match tool {
        coco_types::ToolName::Edit | coco_types::ToolName::Write => input
            .get("file_path")
            .and_then(|v| v.as_str())
            .into_iter()
            .collect(),
        coco_types::ToolName::NotebookEdit => input
            .get("notebook_path")
            .and_then(|v| v.as_str())
            .into_iter()
            .collect(),
        coco_types::ToolName::ApplyPatch => {
            let patch = input.get("patch").and_then(|v| v.as_str())?;
            coco_types::tool_summary::apply_patch_target_paths(patch)
        }
        _ => return None,
    };
    if paths.is_empty() {
        return None;
    }
    let mut rule_contents = std::collections::BTreeSet::new();
    for path in paths {
        for dir in rule_dirs(path, cwd) {
            rule_contents.insert(format!("{dir}/**"));
        }
    }
    if rule_contents.is_empty() {
        return None;
    }
    let rules = rule_contents
        .into_iter()
        .map(|rule_content| coco_types::PermissionRule {
            source,
            behavior: coco_types::PermissionBehavior::Allow,
            value: coco_types::PermissionRuleValue {
                tool_pattern: coco_types::ToolName::Edit.as_str().to_string(),
                rule_content: Some(rule_content),
            },
        })
        .collect();
    Some(coco_types::PermissionUpdate::AddRules { rules, destination })
}

fn read_path_allow_update(
    tool_name: &str,
    original_input: Option<&serde_json::Value>,
    cwd: Option<&str>,
    source: coco_types::PermissionRuleSource,
    destination: coco_types::PermissionUpdateDestination,
) -> Option<coco_types::PermissionUpdate> {
    let tool = coco_types::ToolName::from_str(tool_name).ok()?;
    if !matches!(
        tool,
        coco_types::ToolName::Read | coco_types::ToolName::Grep | coco_types::ToolName::Glob
    ) {
        return None;
    }
    let input = original_input?;
    let raw_path = match tool {
        coco_types::ToolName::Read => input.get("file_path").and_then(|v| v.as_str())?,
        coco_types::ToolName::Grep | coco_types::ToolName::Glob => {
            input.get("path").and_then(|v| v.as_str())?
        }
        _ => return None,
    };
    let rules: Vec<_> = rule_dirs(raw_path, cwd)
        .into_iter()
        .map(|dir| coco_types::PermissionRule {
            source,
            behavior: coco_types::PermissionBehavior::Allow,
            value: coco_types::PermissionRuleValue {
                tool_pattern: coco_types::ToolName::Read.as_str().to_string(),
                rule_content: Some(format!("{dir}/**")),
            },
        })
        .collect();
    if rules.is_empty() {
        return None;
    }
    Some(coco_types::PermissionUpdate::AddRules { rules, destination })
}

/// Resolve a tool-input path to its scoped rule directories, matching the
/// engine's `read_permission_suggestions` byte-for-byte: relative paths join
/// `cwd` (fail-closed when absent), then `get_paths_for_permission_check`
/// lexically normalizes (`..`) and walks the symlink chain so the TUI's
/// `LocalSettings` grant covers the same directories as core's `Session`
/// suggestion. Returns the `//abs/path`-prefixed rule directories (no
/// filesystem root).
fn rule_dirs(raw_path: &str, cwd: Option<&str>) -> Vec<String> {
    let Some(dir) = directory_for_permission_rule(raw_path, cwd) else {
        return Vec::new();
    };
    // Absolute `dir` ignores cwd inside the resolver; `/` is a harmless filler
    // for the absolute case and the only reachable value when cwd is present.
    let cwd = cwd.unwrap_or("/");
    coco_permissions::filesystem::get_paths_for_permission_check(&dir, cwd)
        .into_iter()
        .filter(|resolved| std::path::Path::new(resolved).parent().is_some())
        .map(|resolved| path_for_permission_rule(std::path::Path::new(&resolved)))
        .collect()
}

fn directory_for_permission_rule(raw_path: &str, cwd: Option<&str>) -> Option<String> {
    let path = shellexpand_read_path(raw_path);
    let absolute = if path.is_absolute() {
        path
    } else {
        std::path::PathBuf::from(cwd?).join(path)
    };
    let dir = if absolute.is_dir() {
        absolute
    } else {
        absolute.parent()?.to_path_buf()
    };
    Some(dir.to_string_lossy().into_owned())
}

fn shellexpand_read_path(raw_path: &str) -> std::path::PathBuf {
    if raw_path == "~" {
        return dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from(raw_path));
    }
    if let Some(rest) = raw_path.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    std::path::PathBuf::from(raw_path)
}

fn path_for_permission_rule(path: &std::path::Path) -> String {
    let path = path.to_string_lossy().replace('\\', "/");
    if path.starts_with('/') {
        format!("/{path}")
    } else {
        path
    }
}

#[cfg(test)]
#[path = "permission_options.test.rs"]
mod tests;
