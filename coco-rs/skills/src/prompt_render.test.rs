use super::*;

// ───────── parse_arguments ─────────

#[test]
fn parse_arguments_simple_split() {
    assert_eq!(parse_arguments("foo bar baz"), vec!["foo", "bar", "baz"]);
}

#[test]
fn parse_arguments_double_quoted() {
    assert_eq!(
        parse_arguments(r#"foo "hello world" baz"#),
        vec!["foo", "hello world", "baz"]
    );
}

#[test]
fn parse_arguments_single_quoted() {
    assert_eq!(
        parse_arguments(r#"foo 'hello world' baz"#),
        vec!["foo", "hello world", "baz"]
    );
}

#[test]
fn parse_arguments_backslash_escape() {
    assert_eq!(parse_arguments(r"foo\ bar baz"), vec!["foo bar", "baz"]);
}

#[test]
fn parse_arguments_empty() {
    let empty: Vec<String> = vec![];
    assert_eq!(parse_arguments(""), empty);
    assert_eq!(parse_arguments("   "), empty);
}

// ───────── substitute_arguments ─────────

#[test]
fn substitute_returns_unchanged_when_args_none() {
    let out = substitute_arguments("hello $0", None, &[], true);
    assert_eq!(out, "hello $0");
}

#[test]
fn substitute_dollar_arguments_expands_to_full_string() {
    let out = substitute_arguments("run $ARGUMENTS now", Some("the test suite"), &[], true);
    assert_eq!(out, "run the test suite now");
}

#[test]
fn substitute_zero_indexed_positional() {
    let out = substitute_arguments("first=$0 second=$1", Some("alpha beta"), &[], false);
    assert_eq!(out, "first=alpha second=beta");
}

#[test]
fn substitute_arguments_indexed_brackets() {
    let out = substitute_arguments(
        "x=$ARGUMENTS[0] y=$ARGUMENTS[1]",
        Some("alpha beta"),
        &[],
        false,
    );
    assert_eq!(out, "x=alpha y=beta");
}

#[test]
fn substitute_named_with_word_boundary() {
    let out = substitute_arguments(
        "$user did $action",
        Some("alice deploy"),
        &["user".to_string(), "action".to_string()],
        false,
    );
    assert_eq!(out, "alice did deploy");
}

#[test]
fn substitute_named_does_not_match_partial() {
    // $foo must NOT match $foobar.
    let out = substitute_arguments(
        "$user vs $username",
        Some("alice"),
        &["user".to_string()],
        false,
    );
    assert_eq!(out, "alice vs $username");
}

#[test]
fn substitute_drops_unfilled_to_empty() {
    let out = substitute_arguments("first=$0 second=$1 third=$2", Some("alpha"), &[], false);
    assert_eq!(out, "first=alpha second= third=");
}

#[test]
fn substitute_double_digit_index() {
    // Uses regex \d+ so $10 works.
    let out = substitute_arguments("tenth=$9", Some("a b c d e f g h i j"), &[], false);
    assert_eq!(out, "tenth=j");
}

#[test]
fn substitute_appends_when_no_placeholder() {
    let out = substitute_arguments("Plain prompt body.", Some("extra payload"), &[], true);
    assert_eq!(out, "Plain prompt body.\n\nARGUMENTS: extra payload");
}

#[test]
fn substitute_no_append_when_args_empty() {
    let out = substitute_arguments("Plain prompt body.", Some(""), &[], true);
    assert_eq!(out, "Plain prompt body.");
}

#[test]
fn substitute_no_append_when_disabled() {
    let out = substitute_arguments("Plain.", Some("extra"), &[], false);
    assert_eq!(out, "Plain.");
}

#[test]
fn substitute_quoted_args_keep_groups_intact() {
    // The shell-quote parse means $0 = "hello world" (one arg).
    let out = substitute_arguments(
        "first=$0 second=$1",
        Some(r#""hello world" tail"#),
        &[],
        false,
    );
    assert_eq!(out, "first=hello world second=tail");
}

#[test]
fn substitute_named_skips_numeric_names() {
    // Names that are pure digits are filtered (would conflict with $N shorthand).
    let out = substitute_arguments(
        "$1 vs $foo",
        Some("alpha"),
        &["1".to_string(), "foo".to_string()],
        false,
    );
    // $1 is positional → parsed[1] is empty (only "alpha" provided);
    // numeric name "1" should NOT cause $1 to be treated as named[0] = "alpha".
    // $foo is named[1] → parsed[1] = "" (only one arg).
    assert_eq!(out, " vs ");
}

// ───────── render_skill_prompt smoke ─────────

#[tokio::test]
async fn render_text_with_args() {
    use crate::SkillContext;
    use crate::SkillSource;
    let skill = SkillDefinition {
        name: "demo".into(),
        display_name: None,
        description: "demo".into(),
        prompt: "Hello $name!".into(),
        source: SkillSource::Bundled,
        aliases: vec![],
        allowed_tools: None,
        model: None,
        model_role: None,
        when_to_use: None,
        argument_names: vec!["name".into()],
        paths: vec![],
        effort: None,
        context: SkillContext::Inline,
        agent: None,
        version: None,
        disabled: false,
        hooks: None,
        argument_hint: None,
        user_invocable: true,
        disable_model_invocation: false,
        shell: None,
        content_length: 0,
        has_user_specified_description: true,
        progress_message: Some("running".to_string()),
        is_hidden: false,
        gated_by: None,
        files: std::collections::HashMap::new(),
        skill_root: None,
    };
    let parts = render_skill_prompt(
        &skill,
        "world",
        &RenderContext {
            allow_shell: false,
            env: vec![],
        },
    )
    .await;
    match &parts[0] {
        PromptPart::Text { text } => assert_eq!(text, "Hello world!"),
        _ => panic!("expected text part"),
    }
}

#[test]
fn test_substitute_skill_env_replaces_dir_and_session() {
    let text = "Assets in ${CLAUDE_SKILL_DIR}; session ${CLAUDE_SESSION_ID}.";
    let out = substitute_skill_env(text, Some("/home/u/.coco/skills/x"), Some("sess-123"));
    assert_eq!(out, "Assets in /home/u/.coco/skills/x; session sess-123.");
}

#[test]
fn test_substitute_skill_env_normalizes_backslashes_and_keeps_unknown() {
    // Windows-style dir is normalized to forward slashes; a None session id
    // leaves the placeholder untouched.
    let out = substitute_skill_env(
        "${CLAUDE_SKILL_DIR} and ${CLAUDE_SESSION_ID}",
        Some(r"C:\skills\x"),
        None,
    );
    assert_eq!(out, "C:/skills/x and ${CLAUDE_SESSION_ID}");
}
