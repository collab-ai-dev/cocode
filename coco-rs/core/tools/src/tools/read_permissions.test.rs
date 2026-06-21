//! R6-T20: file_read_ignore_matcher_from_patterns + permission helpers.
//!
//! Tests construct a matcher directly from pattern strings so they
//! don't touch process env.

use super::*;
use coco_tool_runtime::ToolUseContext;
use coco_types::AdditionalWorkingDir;
use coco_types::PermissionBehavior;
use coco_types::PermissionRule;
use coco_types::PermissionRuleSource;
use coco_types::PermissionRuleValue;
use coco_types::ToolCheckResult;

/// Build a matcher from a list of patterns for testing.
fn build_test_matcher(patterns: &[&str]) -> GlobSet {
    let owned: Vec<String> = patterns.iter().map(|s| (*s).to_string()).collect();
    file_read_ignore_matcher_from_patterns(&owned)
}

fn is_ignored_with(patterns: &[&str], path: &str) -> bool {
    let matcher = build_test_matcher(patterns);
    is_read_ignored_with_matcher(Path::new(path), &matcher)
}

/// Create a fixture directory that the permission layer treats as an
/// ordinary path — i.e. NOT one of the `coco_permissions::filesystem`
/// readable-internal exemptions.
///
/// `tempfile::tempdir()` anchors at `$TMPDIR`. Sandboxed runners (incl.
/// this repo's CI image) set `TMPDIR=/tmp/claude-<uid>`, which is the
/// permission layer's project-temp / scratchpad space:
/// `is_readable_internal_path` allows *any* path under the `/tmp/claude-`
/// prefix. A fixture created there reads back as an allowed internal path,
/// so the "outside working dir" / "path traversal" / "rule does not apply"
/// assertions silently flip from `Ask` to `Allow`. On a plain `/tmp`
/// (Linux) or `/var/folders` (macOS) `$TMPDIR` there is no collision, so
/// this only bites under the sandbox TMPDIR — anchoring fixtures under a
/// neutral base makes the tests independent of the runner's `$TMPDIR`.
fn fixture_tempdir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("coco-readperm-")
        .tempdir_in(neutral_temp_root())
        .expect("create fixture tempdir under a neutral temp root")
}

/// Pick a temp root that is not a `coco_permissions` readable-internal
/// exemption. Uses `$TMPDIR` unless it collides with the `/tmp/claude-`
/// project-temp prefix, in which case it falls back to the system root
/// temp (`/tmp` on Unix) — writable and treated as an ordinary path.
fn neutral_temp_root() -> std::path::PathBuf {
    let default = std::env::temp_dir();
    let canonical = std::fs::canonicalize(&default).unwrap_or_else(|_| default.clone());
    let lower = canonical.to_string_lossy().to_lowercase();
    // macOS canonicalizes `/tmp` → `/private/tmp`; compare on the
    // non-`/private` form so the prefix check matches `normalize_for_comparison`.
    let normalized = lower.strip_prefix("/private").unwrap_or(&lower);
    if cfg!(unix) && normalized.starts_with("/tmp/claude-") {
        std::path::PathBuf::from("/tmp")
    } else {
        default
    }
}

/// `.env` pattern catches `.env`, `foo/.env`, `/abs/path/.env`.
#[test]
fn test_dotenv_pattern_catches_all_locations() {
    let patterns = &[".env"];
    assert!(is_ignored_with(patterns, ".env"));
    assert!(is_ignored_with(patterns, "foo/.env"));
    assert!(is_ignored_with(patterns, "/abs/path/.env"));
    assert!(is_ignored_with(patterns, "a/b/c/.env"));
}

/// Glob pattern with wildcard: `*.key` matches any `.key` file.
#[test]
fn test_wildcard_pattern() {
    let patterns = &["*.key"];
    assert!(is_ignored_with(patterns, "private.key"));
    assert!(is_ignored_with(patterns, "ssh_host_rsa.key"));
}

/// Directory pattern: `secrets/*` matches files inside `secrets/`.
#[test]
fn test_directory_pattern() {
    let patterns = &["secrets/*"];
    assert!(is_ignored_with(patterns, "secrets/token"));
    assert!(is_ignored_with(patterns, "secrets/prod.json"));
    // Does NOT match files that happen to have `secrets` in the middle.
    assert!(!is_ignored_with(patterns, "my_secrets.txt"));
}

/// Empty patterns list → nothing is blocked.
#[test]
fn test_empty_patterns_allow_everything() {
    let patterns: &[&str] = &[];
    assert!(!is_ignored_with(patterns, ".env"));
    assert!(!is_ignored_with(patterns, "secrets/token"));
    assert!(!is_ignored_with(patterns, "private.key"));
}

/// `check_read_permission_with_matcher` allows unignored cwd paths.
#[test]
fn test_check_read_permission_allows_unignored_cwd_path() {
    let matcher = build_test_matcher(&[]);
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(std::env::current_dir().unwrap());
    let result = check_read_permission_with_matcher(Path::new("src/main.rs"), &matcher, &ctx);
    assert!(matches!(result, ToolCheckResult::Allow { .. }));
}

/// Unicode + path with special chars: glob matching should still work
/// against the path bytes.
#[test]
fn test_non_ascii_path() {
    let patterns = &["*.secret"];
    assert!(is_ignored_with(patterns, "配置.secret"));
    assert!(is_ignored_with(patterns, "мой.secret"));
}

#[test]
fn test_read_permission_allows_path_inside_cwd() {
    let dir = fixture_tempdir();
    let file = dir.path().join("src/lib.rs");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "mod tests;").unwrap();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(dir.path().to_path_buf());

    let result = check_read_permission_for_path(file.to_str().unwrap(), &ctx);

    assert!(matches!(result, ToolCheckResult::Allow { .. }));
}

#[test]
fn test_read_permission_asks_for_path_outside_working_dirs() {
    let cwd = fixture_tempdir();
    let outside = fixture_tempdir();
    let file = outside.path().join("secret.txt");
    std::fs::write(&file, "secret").unwrap();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(cwd.path().to_path_buf());

    let result = check_read_permission_for_path(file.to_str().unwrap(), &ctx);

    assert!(matches!(result, ToolCheckResult::Ask { .. }));
}

#[test]
fn test_read_permission_asks_for_path_traversal_outside_cwd() {
    let parent = fixture_tempdir();
    let cwd = parent.path().join("repo");
    std::fs::create_dir_all(&cwd).unwrap();
    let outside = parent.path().join("secret.txt");
    std::fs::write(&outside, "secret").unwrap();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(cwd);

    let result = check_read_permission_for_path("../secret.txt", &ctx);

    assert!(matches!(result, ToolCheckResult::Ask { .. }));
}

#[test]
fn test_read_permission_asks_for_suspicious_windows_path_inside_cwd() {
    let dir = fixture_tempdir();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(dir.path().to_path_buf());

    let result = check_read_permission_for_path("GIT~1/config", &ctx);

    assert!(matches!(result, ToolCheckResult::Ask { .. }));
}

#[test]
fn test_read_permission_ask_includes_path_scoped_suggestion() {
    let cwd = fixture_tempdir();
    let outside = fixture_tempdir();
    let file = outside.path().join("secret.txt");
    std::fs::write(&file, "secret").unwrap();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(cwd.path().to_path_buf());

    let result = check_read_permission_for_path(file.to_str().unwrap(), &ctx);

    let ToolCheckResult::Ask { suggestions, .. } = result else {
        panic!("expected ask");
    };
    assert!(suggestions.iter().any(|update| match update {
        coco_types::PermissionUpdate::AddRules { rules, destination } => {
            *destination == coco_types::PermissionUpdateDestination::Session
                && rules.iter().any(|rule| {
                    rule.value.tool_pattern == "Read"
                        && rule.value.rule_content.as_deref().is_some_and(|content| {
                            content.starts_with("//") && content.ends_with("/**")
                        })
                })
        }
        _ => false,
    }));
}

#[test]
fn test_read_permission_allows_additional_working_dir() {
    let cwd = fixture_tempdir();
    let outside = fixture_tempdir();
    let file = outside.path().join("data.txt");
    std::fs::write(&file, "data").unwrap();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(cwd.path().to_path_buf());
    ctx.permission_context.additional_dirs.insert(
        outside.path().to_string_lossy().to_string(),
        AdditionalWorkingDir {
            path: outside.path().to_string_lossy().to_string(),
            source: coco_types::PermissionUpdateDestination::Session,
        },
    );

    let result = check_read_permission_for_path(file.to_str().unwrap(), &ctx);

    assert!(matches!(result, ToolCheckResult::Allow { .. }));
}

#[test]
fn test_read_permission_honors_path_scoped_read_allow_rule() {
    let cwd = fixture_tempdir();
    let outside = fixture_tempdir();
    let file = outside.path().join("data.txt");
    std::fs::write(&file, "data").unwrap();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(cwd.path().to_path_buf());
    let rule_content = format!("/{}/**", outside.path().to_string_lossy());
    ctx.permission_context.allow_rules.insert(
        PermissionRuleSource::Session,
        vec![PermissionRule {
            source: PermissionRuleSource::Session,
            behavior: PermissionBehavior::Allow,
            value: PermissionRuleValue {
                tool_pattern: "Read".to_string(),
                rule_content: Some(rule_content),
            },
        }],
    );

    let result = check_read_permission_for_path(file.to_str().unwrap(), &ctx);

    assert!(matches!(result, ToolCheckResult::Allow { .. }));
}

#[test]
fn test_read_permission_edit_allow_implies_read_allow() {
    let cwd = fixture_tempdir();
    let outside = fixture_tempdir();
    let file = outside.path().join("data.txt");
    std::fs::write(&file, "data").unwrap();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(cwd.path().to_path_buf());
    let rule_content = format!("/{}/**", outside.path().to_string_lossy());
    ctx.permission_context.allow_rules.insert(
        PermissionRuleSource::Session,
        vec![PermissionRule {
            source: PermissionRuleSource::Session,
            behavior: PermissionBehavior::Allow,
            value: PermissionRuleValue {
                tool_pattern: "Edit".to_string(),
                rule_content: Some(rule_content),
            },
        }],
    );

    let result = check_read_permission_for_path(file.to_str().unwrap(), &ctx);

    assert!(matches!(result, ToolCheckResult::Allow { .. }));
}

#[test]
fn test_read_permission_apply_patch_allow_does_not_imply_read_allow() {
    let cwd = fixture_tempdir();
    let outside = fixture_tempdir();
    let file = outside.path().join("data.txt");
    std::fs::write(&file, "data").unwrap();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(cwd.path().to_path_buf());
    ctx.permission_context.allow_rules.insert(
        PermissionRuleSource::Session,
        vec![PermissionRule {
            source: PermissionRuleSource::Session,
            behavior: PermissionBehavior::Allow,
            value: PermissionRuleValue {
                tool_pattern: "apply_patch".to_string(),
                rule_content: None,
            },
        }],
    );

    let result = check_read_permission_for_path(file.to_str().unwrap(), &ctx);

    assert!(matches!(result, ToolCheckResult::Ask { .. }));
}

#[test]
fn test_read_permission_honors_ts_double_slash_read_allow_rule() {
    let cwd = fixture_tempdir();
    let outside = fixture_tempdir();
    let file = outside.path().join("data.txt");
    std::fs::write(&file, "data").unwrap();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(cwd.path().to_path_buf());
    let rule_content = format!("/{}/**", outside.path().to_string_lossy());
    ctx.permission_context.allow_rules.insert(
        PermissionRuleSource::Session,
        vec![PermissionRule {
            source: PermissionRuleSource::Session,
            behavior: PermissionBehavior::Allow,
            value: PermissionRuleValue {
                tool_pattern: "Read".to_string(),
                rule_content: Some(rule_content),
            },
        }],
    );

    let result = check_read_permission_for_path(file.to_str().unwrap(), &ctx);

    assert!(matches!(result, ToolCheckResult::Allow { .. }));
}

#[test]
fn test_read_permission_single_slash_rule_is_source_root_relative() {
    let cwd = fixture_tempdir();
    let outside = fixture_tempdir();
    let file = outside.path().join("data.txt");
    std::fs::write(&file, "data").unwrap();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(cwd.path().to_path_buf());
    ctx.permission_context.allow_rules.insert(
        PermissionRuleSource::Session,
        vec![PermissionRule {
            source: PermissionRuleSource::Session,
            behavior: PermissionBehavior::Allow,
            value: PermissionRuleValue {
                tool_pattern: "Read".to_string(),
                rule_content: Some(format!("{}/**", outside.path().to_string_lossy())),
            },
        }],
    );

    let result = check_read_permission_for_path(file.to_str().unwrap(), &ctx);

    assert!(matches!(result, ToolCheckResult::Ask { .. }));
}
