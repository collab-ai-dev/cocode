use super::*;

/// Test-only convenience wrapper exercising the production `expand_skill_prompt`
/// with only positional / `$ARGUMENTS` substitution (no dir/session/names). The
/// production entry point is `expand_skill_prompt`; this keeps the
/// expansion-behavior tests focused without hand-building `ExpandOptions`.
fn expand_skill_prompt_simple(template: &str, args: &str) -> String {
    expand_skill_prompt(
        template,
        &ExpandOptions {
            args,
            argument_names: &[],
            skill_dir: None,
            session_id: None,
            base_dir: None,
            plugin_root: None,
            plugin_data_dir: None,
            user_config: None,
        },
    )
}

// ── Argument expansion tests ──

#[test]
fn test_expand_skill_prompt_no_placeholders() {
    assert_eq!(
        expand_skill_prompt_simple("Do the thing", ""),
        "Do the thing"
    );
    assert_eq!(
        expand_skill_prompt_simple("Do the thing", "with args"),
        "Do the thing\n\nARGUMENTS: with args"
    );
}

#[test]
fn test_expand_skill_prompt_arguments_placeholder() {
    let result = expand_skill_prompt_simple("Review $ARGUMENTS for issues", "src/main.rs");
    assert_eq!(result, "Review src/main.rs for issues");
}

#[test]
fn test_expand_skill_prompt_braced_arguments() {
    let result = expand_skill_prompt_simple("Fix ${ARGUMENTS} now", "the bug");
    assert_eq!(result, "Fix the bug now");
}

#[test]
fn test_expand_skill_prompt_positional_args() {
    // $N is zero-indexed — alias for $ARGUMENTS[N]. So `$0` is the
    // first parsed arg, `$1` is the second.
    let result = expand_skill_prompt_simple("Compare $0 with $1", "old.rs new.rs");
    assert_eq!(result, "Compare old.rs with new.rs");
}

#[test]
fn test_expand_skill_prompt_positional_braced() {
    let result = expand_skill_prompt_simple("File: ${0}, line: ${1}", "main.rs 42");
    assert_eq!(result, "File: main.rs, line: 42");
}

#[test]
fn test_expand_skill_prompt_unused_positional_cleared() {
    // args "only one" → parsedArgs = ["only", "one"]:
    //   $0 = "only", $1 = "one", $2 = "" (out of range).
    let result = expand_skill_prompt_simple("$0 and $1 and $2", "only one");
    assert_eq!(result, "only and one and ");
}

// ── Variable substitution tests ──

#[test]
fn test_expand_skill_prompt_claude_skill_dir() {
    let result = expand_skill_prompt(
        "Run ${CLAUDE_SKILL_DIR}/helper.sh",
        &ExpandOptions {
            args: "",
            argument_names: &[],
            skill_dir: Some("/home/user/.coco/skills/my-skill"),
            session_id: None,
            base_dir: None,
            plugin_root: None,
            plugin_data_dir: None,
            user_config: None,
        },
    );
    assert_eq!(result, "Run /home/user/.coco/skills/my-skill/helper.sh");
}

#[test]
fn test_expand_skill_prompt_claude_session_id() {
    let result = expand_skill_prompt(
        "Session: ${CLAUDE_SESSION_ID}",
        &ExpandOptions {
            args: "",
            argument_names: &[],
            skill_dir: None,
            session_id: Some("abc-123"),
            base_dir: None,
            plugin_root: None,
            plugin_data_dir: None,
            user_config: None,
        },
    );
    assert_eq!(result, "Session: abc-123");
}

#[test]
fn test_expand_skill_prompt_all_variables() {
    let result = expand_skill_prompt(
        "Dir: ${CLAUDE_SKILL_DIR}, Session: ${CLAUDE_SESSION_ID}, Args: $ARGUMENTS",
        &ExpandOptions {
            args: "hello world",
            argument_names: &[],
            skill_dir: Some("/skills/test"),
            session_id: Some("sess-1"),
            base_dir: None,
            plugin_root: None,
            plugin_data_dir: None,
            user_config: None,
        },
    );
    assert_eq!(
        result,
        "Dir: /skills/test, Session: sess-1, Args: hello world"
    );
}

#[test]
fn test_expand_skill_prompt_no_skill_dir_leaves_placeholder() {
    let result = expand_skill_prompt(
        "Dir: ${CLAUDE_SKILL_DIR}",
        &ExpandOptions {
            args: "",
            argument_names: &[],
            skill_dir: None,
            session_id: None,
            base_dir: None,
            plugin_root: None,
            plugin_data_dir: None,
            user_config: None,
        },
    );
    // Without skill_dir, the placeholder remains
    assert_eq!(result, "Dir: ${CLAUDE_SKILL_DIR}");
}

// ── Named argument tests ──

#[test]
fn test_expand_skill_prompt_named_args() {
    let names = vec!["env".to_string(), "region".to_string()];
    let result = expand_skill_prompt(
        "Deploy to $env in $region",
        &ExpandOptions {
            args: "prod us-east-1",
            argument_names: &names,
            skill_dir: None,
            session_id: None,
            base_dir: None,
            plugin_root: None,
            plugin_data_dir: None,
            user_config: None,
        },
    );
    assert_eq!(result, "Deploy to prod in us-east-1");
}

#[test]
fn test_expand_skill_prompt_indexed_arguments() {
    let result =
        expand_skill_prompt_simple("First: $ARGUMENTS[0], Second: $ARGUMENTS[1]", "foo bar");
    assert_eq!(result, "First: foo, Second: bar");
}

// ── Base directory prepend tests ──

#[test]
fn test_expand_skill_prompt_base_dir() {
    let result = expand_skill_prompt(
        "Do stuff",
        &ExpandOptions {
            args: "",
            argument_names: &[],
            skill_dir: None,
            session_id: None,
            base_dir: Some("/home/user/.coco/skills/my-skill"),
            plugin_root: None,
            plugin_data_dir: None,
            user_config: None,
        },
    );
    assert!(
        result.starts_with("Base directory for this skill: /home/user/.coco/skills/my-skill\n\n")
    );
    assert!(result.ends_with("Do stuff"));
}

#[test]
fn test_expand_skill_prompt_base_dir_empty_skipped() {
    let result = expand_skill_prompt(
        "Do stuff",
        &ExpandOptions {
            args: "",
            argument_names: &[],
            skill_dir: None,
            session_id: None,
            base_dir: Some(""),
            plugin_root: None,
            plugin_data_dir: None,
            user_config: None,
        },
    );
    assert_eq!(result, "Do stuff");
}

// ── Skill name tests ──

#[test]
fn test_normalize_skill_name() {
    assert_eq!(normalize_skill_name("/commit"), "commit");
    assert_eq!(normalize_skill_name("commit"), "commit");
    assert_eq!(normalize_skill_name("  /review-pr  "), "review-pr");
}

// ── Plugin variable substitution tests ──

#[test]
fn test_expand_skill_prompt_plugin_root() {
    let result = expand_skill_prompt(
        "Run ${CLAUDE_PLUGIN_ROOT}/scripts/setup.sh",
        &ExpandOptions {
            args: "",
            argument_names: &[],
            skill_dir: None,
            session_id: None,
            base_dir: None,
            plugin_root: Some("/home/user/.coco/plugins/cache/mkt/my-plugin"),
            plugin_data_dir: None,
            user_config: None,
        },
    );
    assert_eq!(
        result,
        "Run /home/user/.coco/plugins/cache/mkt/my-plugin/scripts/setup.sh"
    );
}

#[test]
fn test_expand_skill_prompt_plugin_data() {
    let result = expand_skill_prompt(
        "Save to ${CLAUDE_PLUGIN_DATA}/output.json",
        &ExpandOptions {
            args: "",
            argument_names: &[],
            skill_dir: None,
            session_id: None,
            base_dir: None,
            plugin_root: None,
            plugin_data_dir: Some("/home/user/.coco/plugins/data/my-plugin"),
            user_config: None,
        },
    );
    assert_eq!(
        result,
        "Save to /home/user/.coco/plugins/data/my-plugin/output.json"
    );
}

#[test]
fn test_expand_skill_prompt_user_config_nonsensitive() {
    let config = [("api_url", "https://api.example.com", false)];
    let result = expand_skill_prompt(
        "Connect to ${user_config.api_url}",
        &ExpandOptions {
            args: "",
            argument_names: &[],
            skill_dir: None,
            session_id: None,
            base_dir: None,
            plugin_root: None,
            plugin_data_dir: None,
            user_config: Some(&config),
        },
    );
    assert_eq!(result, "Connect to https://api.example.com");
}

#[test]
fn test_expand_skill_prompt_user_config_sensitive_masked() {
    let config = [("api_key", "sk-secret-123", true)];
    let result = expand_skill_prompt(
        "Auth with ${user_config.api_key}",
        &ExpandOptions {
            args: "",
            argument_names: &[],
            skill_dir: None,
            session_id: None,
            base_dir: None,
            plugin_root: None,
            plugin_data_dir: None,
            user_config: Some(&config),
        },
    );
    assert_eq!(result, "Auth with [SENSITIVE:api_key]");
}

#[test]
fn test_expand_skill_prompt_all_plugin_variables() {
    let config = [
        ("url", "https://example.com", false),
        ("token", "secret", true),
    ];
    let result = expand_skill_prompt(
        "Root: ${CLAUDE_PLUGIN_ROOT}, Data: ${CLAUDE_PLUGIN_DATA}, URL: ${user_config.url}, Token: ${user_config.token}",
        &ExpandOptions {
            args: "",
            argument_names: &[],
            skill_dir: None,
            session_id: None,
            base_dir: None,
            plugin_root: Some("/plugins/test"),
            plugin_data_dir: Some("/data/test"),
            user_config: Some(&config),
        },
    );
    assert_eq!(
        result,
        "Root: /plugins/test, Data: /data/test, URL: https://example.com, Token: [SENSITIVE:token]"
    );
}
