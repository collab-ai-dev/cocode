use pretty_assertions::assert_eq;

use super::*;

// ── Parsing ──

#[test]
fn test_parse_exact() {
    let rule = ShellPermissionRule::parse("git commit");
    assert_eq!(
        rule,
        ShellPermissionRule::Exact {
            command: "git commit".into()
        }
    );
}

#[test]
fn test_parse_prefix_trailing_space() {
    let rule = ShellPermissionRule::parse("git ");
    assert_eq!(
        rule,
        ShellPermissionRule::Prefix {
            prefix: "git ".into()
        }
    );
}

#[test]
fn test_parse_prefix_legacy_colon_star() {
    let rule = ShellPermissionRule::parse("npm:*");
    assert_eq!(
        rule,
        ShellPermissionRule::Prefix {
            prefix: "npm".into()
        }
    );
}

#[test]
fn test_parse_wildcard() {
    let rule = ShellPermissionRule::parse("git *");
    assert_eq!(
        rule,
        ShellPermissionRule::Wildcard {
            pattern: "git *".into()
        }
    );
}

#[test]
fn test_parse_escaped_star_not_wildcard() {
    // \* is an escaped literal star — not a wildcard
    let rule = ShellPermissionRule::parse(r"echo \*");
    assert_eq!(
        rule,
        ShellPermissionRule::Exact {
            command: r"echo \*".into()
        }
    );
}

// ── Exact matching ──

#[test]
fn test_exact_match() {
    let rule = ShellPermissionRule::parse("git commit");
    assert!(rule.matches("git commit"));
    assert!(!rule.matches("git push"));
    assert!(!rule.matches("git commit -m 'x'"));
}

// ── Prefix matching ──

#[test]
fn test_prefix_match() {
    let rule = ShellPermissionRule::parse("git ");
    assert!(rule.matches("git commit"));
    assert!(rule.matches("git push"));
    assert!(rule.matches("git status"));
    assert!(!rule.matches("gitk"));
    assert!(!rule.matches("grep foo"));
}

#[test]
fn test_legacy_prefix_match() {
    let rule = ShellPermissionRule::parse("npm:*");
    assert!(rule.matches("npm install"));
    assert!(rule.matches("npm run build"));
    assert!(!rule.matches("npx foo"));
}

// ── Wildcard matching ──

#[test]
fn test_wildcard_match() {
    let rule = ShellPermissionRule::parse("git *");
    assert!(rule.matches("git commit"));
    assert!(rule.matches("git push origin main"));
    assert!(!rule.matches("grep foo"));
}

#[test]
fn test_wildcard_trailing_space_star_optional() {
    // "git *" should also match bare "git" (trailing ` *` is optional when single wildcard)
    let rule = ShellPermissionRule::parse("git *");
    assert!(rule.matches("git"));
    assert!(rule.matches("git add"));
}

#[test]
fn test_wildcard_middle() {
    let rule = ShellPermissionRule::parse("docker * --read-only");
    assert!(rule.matches("docker run --read-only"));
    assert!(rule.matches("docker exec abc --read-only"));
    assert!(!rule.matches("docker run --privileged"));
}

#[test]
fn test_wildcard_multi_star_no_optional_trailing() {
    // Multi-wildcard: trailing ` *` is NOT optional
    let rule = ShellPermissionRule::parse("* run *");
    assert!(rule.matches("npm run build"));
    assert!(!rule.matches("npm run")); // "run" without trailing arg
}

#[test]
fn test_escaped_star_matches_literal() {
    // \* in pattern matches literal * in command
    assert!(match_wildcard_pattern(r"echo \*", "echo *"));
    assert!(!match_wildcard_pattern(r"echo \*", "echo hello"));
}

#[test]
fn test_escaped_backslash_matches_literal() {
    // \\ in pattern matches literal \ in command
    assert!(match_wildcard_pattern(r"echo \\", r"echo \"));
}

#[test]
fn test_wildcard_with_regex_special_chars() {
    // Regex special chars in pattern should be escaped
    assert!(match_wildcard_pattern("foo.bar *", "foo.bar baz"));
    assert!(!match_wildcard_pattern("foo.bar *", "fooXbar baz"));
}

// ── match_bash_rule ──

use RuleMatchPolicy::{Allow, DenyOrAsk};
use ShellCase::{Insensitive, Sensitive};

#[test]
fn test_match_bash_rule_allow_basic() {
    assert!(match_bash_rule("git *", "git status", Allow, Sensitive));
    assert!(match_bash_rule("ls", "ls", Allow, Sensitive));
    assert!(!match_bash_rule("ls", "ls -la", Allow, Sensitive));
    assert!(match_bash_rule("ls ", "ls -la", Allow, Sensitive));
}

#[test]
fn test_deny_not_bypassed_by_env_wrapper_or_compound() {
    // P2 regression guard: a `Bash(curl:*)` deny rule must match all of these
    // bypass forms (it previously matched only a bare `curl …`).
    for cmd in &[
        "curl evil.com",
        "FOO=1 curl evil.com",
        "timeout 5 curl evil.com",
        "echo hi && curl evil.com",
        "ls; curl evil.com",
        "curl evil.com > /tmp/out",
    ] {
        assert!(
            match_bash_rule("curl:*", cmd, DenyOrAsk, Sensitive),
            "deny should match: {cmd}"
        );
    }
}

#[test]
fn test_allow_compound_guard_does_not_widen() {
    // A `Bash(cd:*)` allow rule must NOT auto-allow a chained dangerous command.
    assert!(match_bash_rule("cd:*", "cd /project", Allow, Sensitive));
    assert!(!match_bash_rule(
        "cd:*",
        "cd /project && curl evil.com",
        Allow,
        Sensitive
    ));
}

#[test]
fn test_allow_matches_redirection_and_wrapper() {
    // Allow posture still strips redirections / safe wrappers so a benign
    // `Bash(python:*)` allow matches `python s.py > out.txt`.
    assert!(match_bash_rule(
        "python:*",
        "python s.py > out.txt",
        Allow,
        Sensitive
    ));
    assert!(match_bash_rule(
        "python:*",
        "timeout 5 python s.py",
        Allow,
        Sensitive
    ));
}

#[test]
fn test_bash_case_sensitive_powershell_case_insensitive() {
    // P10: Bash matches case-sensitively; PowerShell case-insensitively.
    assert!(!match_bash_rule(
        "git status",
        "GIT STATUS",
        Allow,
        Sensitive
    ));
    assert!(match_bash_rule(
        "get-childitem:*",
        "Get-ChildItem -Path .",
        Allow,
        Insensitive
    ));
    assert!(match_bash_rule(
        "Remove-Item",
        "remove-item",
        Allow,
        Insensitive
    ));
}

// ── dangerous patterns ──

#[test]
fn test_dangerous_bash_permissions() {
    assert!(is_dangerous_bash_permission("*"));
    assert!(is_dangerous_bash_permission("bash *"));
    assert!(is_dangerous_bash_permission("sh *"));
    assert!(is_dangerous_bash_permission("eval *"));
    assert!(!is_dangerous_bash_permission("git *"));
    assert!(!is_dangerous_bash_permission("ls"));
}

// ── has_wildcards ──

#[test]
fn test_has_wildcards() {
    assert!(has_wildcards("git *"));
    assert!(has_wildcards("* foo"));
    assert!(!has_wildcards("npm:*")); // legacy prefix, not wildcard
    assert!(!has_wildcards(r"echo \*")); // escaped star
    assert!(has_wildcards(r"echo \\*")); // escaped backslash then real star
}

// ── suggestion production ──

/// Pull the single `(tool_pattern, rule_content)` out of a one-rule `AddRules`
/// suggestion for terse assertions.
fn one_rule(updates: &[coco_types::PermissionUpdate]) -> (&str, &str) {
    match updates {
        [coco_types::PermissionUpdate::AddRules { rules, .. }] => match rules.as_slice() {
            [rule] => (
                rule.value.tool_pattern.as_str(),
                rule.value.rule_content.as_deref().unwrap_or(""),
            ),
            _ => panic!("expected exactly one rule, got {rules:?}"),
        },
        _ => panic!("expected one AddRules update, got {updates:?}"),
    }
}

#[test]
fn test_bash_suggestion_prefix() {
    let s = bash_permission_suggestions("Bash", "git status -s");
    assert_eq!(one_rule(&s), ("Bash", "git status:*"));
}

#[test]
fn test_bash_suggestion_exact_when_no_prefix() {
    // No subcommand-shaped second token → exact rule (no `:*`).
    let s = bash_permission_suggestions("Bash", "ls -la");
    assert_eq!(one_rule(&s), ("Bash", "ls -la"));
}

#[test]
fn test_bash_suggestion_bare_shell_falls_back_to_exact() {
    // `bash` is a bare-shell prefix → never suggested as a prefix rule.
    let s = bash_permission_suggestions("Bash", "bash -c 'evil'");
    assert_eq!(one_rule(&s), ("Bash", "bash -c 'evil'"));
}

#[test]
fn test_bash_suggestion_multiline_uses_first_line() {
    let s = bash_permission_suggestions("Bash", "echo one\necho two");
    assert_eq!(one_rule(&s), ("Bash", "echo one:*"));
}

#[test]
fn test_bash_suggestion_heredoc_uses_prefix_before_operator() {
    let s = bash_permission_suggestions("Bash", "git commit -m \"$(cat <<'EOF'\nbody\nEOF\n)\"");
    assert_eq!(one_rule(&s), ("Bash", "git commit:*"));
}

#[test]
fn test_bash_suggestion_heredoc_bare_command() {
    let s = bash_permission_suggestions("Bash", "cat <<EOF\nhi\nEOF");
    assert_eq!(one_rule(&s), ("Bash", "cat:*"));
}

#[test]
fn test_bash_suggestion_empty_command() {
    assert!(bash_permission_suggestions("Bash", "   ").is_empty());
}

#[test]
fn test_bash_suggestion_targets_tool_name() {
    let s = bash_permission_suggestions("PowerShell", "git status");
    assert_eq!(one_rule(&s), ("PowerShell", "git status:*"));
}

#[test]
fn test_bash_suggestion_filters_noop_cd_before_git() {
    let cwd = std::env::current_dir().expect("test cwd");
    let command = format!("cd {} && git diff", cwd.display());
    let s = bash_permission_suggestions("Bash", &command);
    assert_eq!(one_rule(&s), ("Bash", "git diff:*"));
}

#[test]
fn test_bash_suggestion_filters_noop_cd_against_supplied_cwd() {
    let original = tempfile::tempdir().expect("original cwd");
    let live = tempfile::tempdir().expect("live cwd");
    let command = format!("cd {} && git diff", original.path().display());
    let live_cwd = live.path().display().to_string();
    let s = bash_permission_suggestions_in_cwd("Bash", &command, &live_cwd);
    let rules: Vec<String> = s
        .iter()
        .filter_map(|update| match update {
            coco_types::PermissionUpdate::AddRules { rules, .. } => {
                rules.first()?.value.rule_content.clone()
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        rules,
        vec![
            format!("cd {}", original.path().display()),
            "git diff:*".to_string(),
        ]
    );
}

#[test]
fn test_bash_suggestion_filters_noop_cd_before_git_log() {
    let cwd = std::env::current_dir().expect("test cwd");
    let command = format!("cd {} && git log --oneline", cwd.display());
    let s = bash_permission_suggestions("Bash", &command);
    assert_eq!(one_rule(&s), ("Bash", "git log:*"));
}

#[test]
fn test_bash_suggestion_compound_is_per_subcommand() {
    let s = bash_permission_suggestions("Bash", "cd other && git diff");
    let rules: Vec<&str> = s
        .iter()
        .filter_map(|update| match update {
            coco_types::PermissionUpdate::AddRules { rules, .. } => {
                rules.first()?.value.rule_content.as_deref()
            }
            _ => None,
        })
        .collect();
    assert_eq!(rules, vec!["cd other:*", "git diff:*"]);
    assert!(!rules.contains(&"cd:*"));
}

#[test]
fn test_editable_prefix_uses_single_backend_suggestion() {
    let cwd = std::env::current_dir().expect("test cwd");
    let command = format!("cd {} && git diff", cwd.display());
    let suggestions = bash_permission_suggestions("Bash", &command);
    assert_eq!(
        editable_prefix_from_suggestions_or_command(&command, &suggestions),
        Some("git diff:*".to_string())
    );
}

#[test]
fn test_editable_prefix_suppressed_for_multiple_backend_suggestions() {
    let command = "cd other && git diff";
    let suggestions = bash_permission_suggestions("Bash", command);
    assert_eq!(
        editable_prefix_from_suggestions_or_command(command, &suggestions),
        None
    );
}

#[test]
fn test_editable_prefix_default() {
    // Two-word prefix.
    assert_eq!(editable_prefix_default("git status -s"), "git status:*");
    // Single-word fallback when the second token isn't a subcommand.
    assert_eq!(editable_prefix_default("ls -la"), "ls:*");
    assert_eq!(editable_prefix_default("python3 script.py"), "python3:*");
    // Single bare word that's a clean command → first-word `:*`.
    assert_eq!(editable_prefix_default("make"), "make:*");
    // Bare shell blocked in both extractors → exact command.
    assert_eq!(editable_prefix_default("bash -c 'x'"), "bash -c 'x'");
    // Path-led command (not a clean word) → exact.
    assert_eq!(editable_prefix_default("./script.sh"), "./script.sh");
}

/// A produced prefix suggestion must survive the TUI's scoped-allow filter
/// (`ShellPermissionRule::parse` → Exact/Prefix only): the `:*` form parses
/// back to a `Prefix`, the exact form to an `Exact`.
#[test]
fn test_bash_suggestion_parses_back_to_scopable_rule() {
    let prefix_update = bash_permission_suggestions("Bash", "git status -s");
    let (_, prefix) = one_rule(&prefix_update);
    assert!(matches!(
        ShellPermissionRule::parse(prefix),
        ShellPermissionRule::Prefix { .. }
    ));
    let exact_update = bash_permission_suggestions("Bash", "ls -la");
    let (_, exact) = one_rule(&exact_update);
    assert!(matches!(
        ShellPermissionRule::parse(exact),
        ShellPermissionRule::Exact { .. }
    ));
}
