//! Shared file-read permission checking for Grep / Glob / Read tools.
//!
//! R6-T20. Every file-read tool is routed through the permission check,
//! which consults the session `toolPermissionContext` for deny rules and
//! applies the resulting glob patterns as ripgrep `--glob '!...'` arguments
//! plus hard-fails direct Read calls against blocked paths.
//!
//! coco-rs resolves the deny patterns ahead of time in
//! `coco_config::ToolConfig::file_read_ignore_patterns` (JSON-first,
//! env override via `COCO_FILE_READ_IGNORE_PATTERNS`). Tools build a
//! matcher from `ctx.tool_config.file_read_ignore_patterns` and pass
//! it into the helpers below. There is intentionally **no** global
//! env-only matcher — keeping a single source of truth prevents
//! JSON-configured patterns from silently disagreeing with a cached
//! env-only snapshot.
//!
//! Wiring:
//!
//! * `ReadTool::check_permissions` rejects file_path directly if it
//!   matches any pattern.
//! * `GrepTool::check_permissions` rejects the `path` argument if it
//!   matches; the walker also skips individual files that match during
//!   traversal via `is_read_ignored_with_matcher`.
//! * `GlobTool::check_permissions` mirrors Grep.
//!
//! Not a security boundary — the ignore patterns are a convenience to
//! hide sensitive files from the model, not a guarantee.

use coco_types::ToolCheckResult;
use coco_types::ToolName;
use globset::Glob;
use globset::GlobSet;
use globset::GlobSetBuilder;
use std::path::Path;
use std::path::PathBuf;

/// Compile a `GlobSet` matcher from a list of patterns.
///
/// Unqualified literal patterns (e.g. `.env`) are automatically expanded
/// with an any-ancestor variant (`**/.env`) so a pattern matches every
/// `.env` in the tree, not just one at the root.
pub fn file_read_ignore_matcher_from_patterns(patterns: &[String]) -> GlobSet {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns.iter().map(String::as_str) {
        if let Ok(glob) = Glob::new(pattern) {
            builder.add(glob);
        } else {
            tracing::warn!(
                pattern = %pattern,
                "file_read_ignore_patterns contains an invalid glob; skipping"
            );
        }
        // Also add an any-ancestor version so `.env` matches
        // `foo/.env`, `a/b/.env`, etc.
        if !pattern.contains('/')
            && !pattern.contains('*')
            && let Ok(glob) = Glob::new(&format!("**/{pattern}"))
        {
            builder.add(glob);
        }
    }
    builder.build().unwrap_or_else(|_| GlobSet::empty())
}

/// Test a path against a caller-supplied matcher.
///
/// Returns `true` if the path matches any ignore pattern and should be
/// blocked. Accepts both absolute and relative paths. Matches against
/// the raw path, the file name, and the path with leading `./` stripped
/// so a pattern like `".env"` catches `/abs/path/.env`, `.env`, and
/// `./.env`.
pub fn is_read_ignored_with_matcher(path: &Path, matcher: &GlobSet) -> bool {
    if matcher.is_empty() {
        return false;
    }
    if matcher.is_match(path) {
        return true;
    }
    if let Some(file_name) = path.file_name()
        && matcher.is_match(Path::new(file_name))
    {
        return true;
    }
    // Try path without leading `./` for relative paths.
    if let Some(stripped) = path.to_str().and_then(|s| s.strip_prefix("./"))
        && matcher.is_match(Path::new(stripped))
    {
        return true;
    }
    false
}

/// Apply the file-read ignore list, then run the read path permission
/// pipeline for Read/Grep/Glob.
///
/// R6-T20. Used by `Tool::check_permissions` overrides in Read/Grep/Glob.
pub fn check_read_permission_with_matcher(
    path: &Path,
    matcher: &GlobSet,
    ctx: &coco_tool_runtime::ToolUseContext,
) -> ToolCheckResult {
    if is_read_ignored_with_matcher(path, matcher) {
        return ToolCheckResult::Deny {
            message: format!(
                "Path `{}` is blocked by file-read ignore patterns. \
                 This is a session-level filter intended to keep \
                 sensitive files out of the model's view; adjust \
                 `tool.file_read_ignore_patterns` in settings (or \
                 `COCO_FILE_READ_IGNORE_PATTERNS`) if you need access.",
                path.display()
            ),
        };
    }

    let path = path.to_string_lossy();
    check_read_permission_for_path(&path, ctx)
}

/// Read deny/ask rules win, then working-directory/internal paths are
/// allowed, then explicit read allow rules are honored, otherwise prompt.
///
/// Permanent logging surface (enable with
/// `COCO_LOG=coco_tools::tools::read_permissions=debug` for decisions
/// or `=trace` for the full path / context dump). Every `return`
/// emits one `debug!` line carrying `permission_decision` and `step`
/// so support sessions can grep the log to find why a given read
/// was allowed or asked.
pub fn check_read_permission_for_path(
    path: &str,
    ctx: &coco_tool_runtime::ToolUseContext,
) -> ToolCheckResult {
    let cwd = effective_cwd(ctx);
    let cwd_str = cwd.to_string_lossy();
    let paths_to_check =
        coco_permissions::filesystem::get_paths_for_permission_check(path, &cwd_str);

    tracing::trace!(
        path = %path,
        cwd = %cwd_str,
        paths_to_check = ?paths_to_check,
        additional_dirs = ?ctx
            .permission_context
            .additional_dirs
            .values()
            .map(|d| d.path.as_str())
            .collect::<Vec<_>>(),
        deny_rule_sources = ctx.permission_context.deny_rules.len(),
        ask_rule_sources = ctx.permission_context.ask_rules.len(),
        allow_rule_sources = ctx.permission_context.allow_rules.len(),
        "read_permission: evaluate",
    );

    if paths_to_check
        .iter()
        .any(|p| p.starts_with("//") || p.starts_with("\\\\"))
    {
        tracing::debug!(
            path = %path,
            permission_decision = "ask",
            "read_permission: UNC path requires manual approval",
        );
        return ToolCheckResult::Ask {
            message: format!(
                "Coco requested permissions to read from {path}, which appears to be a UNC path that could access network resources."
            ),
            suggestions: vec![],
            choices: None,
            detail: None,
        };
    }

    if paths_to_check
        .iter()
        .any(|p| coco_permissions::filesystem::has_suspicious_windows_pattern(p))
    {
        tracing::debug!(
            path = %path,
            permission_decision = "ask",
            "read_permission: suspicious Windows path requires manual approval",
        );
        return ToolCheckResult::Ask {
            message: format!(
                "Coco requested permissions to read from {path}, which contains a suspicious Windows path pattern that requires manual approval."
            ),
            suggestions: vec![],
            choices: None,
            detail: None,
        };
    }

    if let Some(rule) = matching_read_rule(
        &ctx.permission_context.deny_rules,
        &paths_to_check,
        &cwd,
        &ctx.permission_context,
    ) {
        tracing::debug!(
            path = %path,
            permission_decision = "deny",
            rule_pattern = %rule.value.tool_pattern,
            rule_content = ?rule.value.rule_content,
            "read_permission: matched deny rule",
        );
        return ToolCheckResult::Deny {
            message: format!(
                "Permission to read {path} has been denied by rule `{}`.",
                rule.value.tool_pattern
            ),
        };
    }

    if let Some(rule) = matching_read_rule(
        &ctx.permission_context.ask_rules,
        &paths_to_check,
        &cwd,
        &ctx.permission_context,
    ) {
        tracing::debug!(
            path = %path,
            permission_decision = "ask",
            rule_pattern = %rule.value.tool_pattern,
            rule_content = ?rule.value.rule_content,
            "read_permission: matched ask rule",
        );
        return ToolCheckResult::Ask {
            message: format!(
                "Coco requested permissions to read from {path}, but you haven't granted it yet."
            ),
            suggestions: vec![],
            choices: None,
            detail: None,
        };
    }

    if let Some(rule) = matching_edit_rule(
        &ctx.permission_context.allow_rules,
        &paths_to_check,
        &cwd,
        &ctx.permission_context,
    ) {
        tracing::debug!(
            path = %path,
            permission_decision = "allow",
            rule_pattern = %rule.value.tool_pattern,
            rule_content = ?rule.value.rule_content,
            "read_permission: edit-allow rule grants read",
        );
        return ToolCheckResult::Allow {
            updated_input: None,
            feedback: None,
        };
    }

    let in_working_dirs = paths_to_check
        .iter()
        .all(|p| path_is_in_working_dirs(p, &cwd_str, ctx));
    let internal_ctx = coco_permissions::filesystem::InternalPathContext {
        cwd: &cwd_str,
        session_plan_file: ctx.permission_context.session_plan_file.as_deref(),
    };
    let internal_readable = paths_to_check
        .iter()
        .any(|p| coco_permissions::filesystem::is_readable_internal_path(p, &internal_ctx));

    if in_working_dirs || internal_readable {
        tracing::debug!(
            path = %path,
            permission_decision = "allow",
            in_working_dirs,
            internal_readable,
            "read_permission: path inside allowed scope",
        );
        return ToolCheckResult::Allow {
            updated_input: None,
            feedback: None,
        };
    }

    if let Some(rule) = matching_read_rule(
        &ctx.permission_context.allow_rules,
        &paths_to_check,
        &cwd,
        &ctx.permission_context,
    ) {
        tracing::debug!(
            path = %path,
            permission_decision = "allow",
            rule_pattern = %rule.value.tool_pattern,
            rule_content = ?rule.value.rule_content,
            "read_permission: matched read-allow rule",
        );
        return ToolCheckResult::Allow {
            updated_input: None,
            feedback: None,
        };
    }

    tracing::debug!(
        path = %path,
        permission_decision = "ask",
        cwd = %cwd_str,
        paths_to_check = ?paths_to_check,
        "read_permission: no rule matched and path outside working dirs",
    );
    ToolCheckResult::Ask {
        message: format!(
            "Coco requested permissions to read from {path}, but you haven't granted it yet."
        ),
        suggestions: read_permission_suggestions(path, &cwd_str),
        choices: None,
        detail: None,
    }
}

fn effective_cwd(ctx: &coco_tool_runtime::ToolUseContext) -> PathBuf {
    ctx.cwd_override
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn path_is_in_working_dirs(path: &str, cwd: &str, ctx: &coco_tool_runtime::ToolUseContext) -> bool {
    if coco_permissions::filesystem::path_in_working_path(path, cwd) {
        tracing::trace!(
            path = %path,
            cwd = %cwd,
            scope = "cwd",
            "read_permission: path matched cwd",
        );
        return true;
    }
    let additional_match = ctx
        .permission_context
        .additional_dirs
        .values()
        .find(|dir| coco_permissions::filesystem::path_in_working_path(path, &dir.path));
    if let Some(dir) = additional_match {
        tracing::trace!(
            path = %path,
            cwd = %cwd,
            scope = "additional_dir",
            matched_dir = %dir.path,
            "read_permission: path matched additional working dir",
        );
        return true;
    }
    tracing::trace!(
        path = %path,
        cwd = %cwd,
        additional_dir_count = ctx.permission_context.additional_dirs.len(),
        "read_permission: path outside cwd and additional dirs",
    );
    false
}

fn matching_read_rule<'a>(
    rules: &'a coco_types::PermissionRulesBySource,
    paths_to_check: &[String],
    cwd: &Path,
    permission_context: &coco_types::ToolPermissionContext,
) -> Option<&'a coco_types::PermissionRule> {
    let match_context = coco_permissions::FileRuleMatchContext::new(cwd)
        .with_source_roots(&permission_context.permission_rule_source_roots);
    coco_permissions::matching_file_rule(
        rules,
        paths_to_check,
        coco_permissions::FileRuleToolType::Read,
        &match_context,
    )
}

pub(crate) fn matching_edit_rule<'a>(
    rules: &'a coco_types::PermissionRulesBySource,
    paths_to_check: &[String],
    cwd: &Path,
    permission_context: &coco_types::ToolPermissionContext,
) -> Option<&'a coco_types::PermissionRule> {
    let match_context = coco_permissions::FileRuleMatchContext::new(cwd)
        .with_source_roots(&permission_context.permission_rule_source_roots);
    coco_permissions::matching_file_rule(
        rules,
        paths_to_check,
        coco_permissions::FileRuleToolType::Edit,
        &match_context,
    )
}

fn read_permission_suggestions(path: &str, cwd: &str) -> Vec<coco_types::PermissionUpdate> {
    let Some(dir) = directory_for_path(path, cwd) else {
        return vec![];
    };
    coco_permissions::filesystem::get_paths_for_permission_check(&dir.to_string_lossy(), cwd)
        .into_iter()
        .filter_map(|dir| {
            let path = PathBuf::from(dir);
            (path.parent().is_some()).then(|| coco_types::PermissionUpdate::AddRules {
                rules: vec![coco_types::PermissionRule {
                    source: coco_types::PermissionRuleSource::Session,
                    behavior: coco_types::PermissionBehavior::Allow,
                    value: coco_types::PermissionRuleValue {
                        tool_pattern: ToolName::Read.as_str().to_string(),
                        rule_content: Some(format!("{}/**", path_for_rule(&path))),
                    },
                }],
                destination: coco_types::PermissionUpdateDestination::Session,
            })
        })
        .collect()
}

fn directory_for_path(path: &str, cwd: &str) -> Option<PathBuf> {
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
    Some(dir)
}

fn path_for_rule(path: &Path) -> String {
    let path = path.to_string_lossy().replace('\\', "/");
    if path.starts_with('/') {
        format!("/{path}")
    } else {
        path
    }
}

#[cfg(test)]
#[path = "read_permissions.test.rs"]
mod tests;
