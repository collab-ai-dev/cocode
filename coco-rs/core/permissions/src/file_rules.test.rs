use super::*;
use coco_types::PermissionBehavior;
use coco_types::PermissionRuleValue;
use pretty_assertions::assert_eq;

fn rule(source: PermissionRuleSource, tool: &str, content: &str) -> PermissionRule {
    PermissionRule {
        source,
        behavior: PermissionBehavior::Allow,
        value: PermissionRuleValue {
            tool_pattern: tool.to_string(),
            rule_content: Some(content.to_string()),
        },
    }
}

#[test]
fn test_double_slash_pattern_roots_at_filesystem_root() {
    let ctx = FileRuleMatchContext::new("/repo");
    let rule = rule(PermissionRuleSource::Session, "Read", "//tmp/secrets/**");

    assert!(file_rule_matches_paths(
        &rule,
        &["/tmp/secrets/token".to_string()],
        FileRuleToolType::Read,
        &ctx
    ));
}

#[test]
fn test_single_slash_pattern_roots_at_source_root() {
    let ctx = FileRuleMatchContext::new("/repo");
    let rule = rule(PermissionRuleSource::Session, "Read", "/tmp/secrets/**");

    assert!(!file_rule_matches_paths(
        &rule,
        &["/tmp/secrets/token".to_string()],
        FileRuleToolType::Read,
        &ctx
    ));
    assert!(file_rule_matches_paths(
        &rule,
        &["/repo/tmp/secrets/token".to_string()],
        FileRuleToolType::Read,
        &ctx
    ));
}

#[test]
fn test_tilde_pattern_roots_at_home() {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let home = PathBuf::from(home);
    let ctx = FileRuleMatchContext::new("/repo");
    let rule = rule(PermissionRuleSource::UserSettings, "Read", "~/secrets/**");
    let file = home.join("secrets/token").to_string_lossy().to_string();

    assert!(file_rule_matches_paths(
        &rule,
        &[file],
        FileRuleToolType::Read,
        &ctx
    ));
}

#[test]
fn test_dot_slash_pattern_matches_cwd_relative_path() {
    let ctx = FileRuleMatchContext::new("/repo");
    let rule = rule(PermissionRuleSource::Session, "Read", "./.env");

    assert!(file_rule_matches_paths(
        &rule,
        &["/repo/.env".to_string()],
        FileRuleToolType::Read,
        &ctx
    ));
}

#[test]
fn test_globstar_pattern_matches_relative_path() {
    let ctx = FileRuleMatchContext::new("/repo");
    let rule = rule(PermissionRuleSource::Session, "Read", "src/**/*.rs");

    assert!(file_rule_matches_paths(
        &rule,
        &["/repo/src/nested/lib.rs".to_string()],
        FileRuleToolType::Read,
        &ctx
    ));
    assert!(!file_rule_matches_paths(
        &rule,
        &["/repo/tests/lib.rs".to_string()],
        FileRuleToolType::Read,
        &ctx
    ));
}

#[test]
fn test_edit_tool_type_uses_canonical_edit_rule() {
    let ctx = FileRuleMatchContext::new("/repo");
    let rule = rule(PermissionRuleSource::Session, "Edit", "/src/**");

    assert!(file_rule_matches_paths(
        &rule,
        &["/repo/src/main.rs".to_string()],
        FileRuleToolType::Edit,
        &ctx
    ));
}

#[test]
fn test_edit_tool_type_does_not_match_write_or_notebook_edit_path_rule() {
    let ctx = FileRuleMatchContext::new("/repo");

    for tool in ["Write", "NotebookEdit", "apply_patch"] {
        let rule = rule(PermissionRuleSource::Session, tool, "/src/**");
        assert_eq!(
            file_rule_matches_paths(
                &rule,
                &["/repo/src/main.rs".to_string()],
                FileRuleToolType::Edit,
                &ctx
            ),
            false,
            "{tool}(...) path-scoped rules must not replace TS Edit(...)"
        );
    }
}

#[test]
fn test_single_slash_pattern_uses_source_root_override() {
    let ctx = FileRuleMatchContext::new("/repo")
        .with_source_root(PermissionRuleSource::UserSettings, "/home/me/.coco");
    let rule = rule(PermissionRuleSource::UserSettings, "Read", "/commands/**");

    assert!(file_rule_matches_paths(
        &rule,
        &["/home/me/.coco/commands/status.md".to_string()],
        FileRuleToolType::Read,
        &ctx
    ));
    assert!(!file_rule_matches_paths(
        &rule,
        &["/repo/commands/status.md".to_string()],
        FileRuleToolType::Read,
        &ctx
    ));
}

#[test]
fn test_read_tool_type_does_not_match_edit_rule() {
    let ctx = FileRuleMatchContext::new("/repo");
    let rule = rule(PermissionRuleSource::Session, "Edit", "/src/**");

    assert_eq!(
        file_rule_matches_paths(
            &rule,
            &["/repo/src/main.rs".to_string()],
            FileRuleToolType::Read,
            &ctx
        ),
        false
    );
}
