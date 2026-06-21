use super::*;

#[test]
fn test_strip_safe_env_vars() {
    assert_eq!(
        strip_safe_wrappers("RUST_LOG=debug cargo test"),
        "cargo test"
    );
    assert_eq!(
        strip_safe_wrappers("NO_COLOR=1 RUST_BACKTRACE=1 cargo build"),
        "cargo build"
    );
}

#[test]
fn test_strip_unsafe_env_var_kept() {
    // LD_PRELOAD is not safe — should NOT be stripped
    assert_eq!(
        strip_safe_wrappers("LD_PRELOAD=/evil.so cat /etc/passwd"),
        "LD_PRELOAD=/evil.so cat /etc/passwd"
    );
}

#[test]
fn test_strip_wrappers() {
    assert_eq!(strip_safe_wrappers("nohup cargo build"), "cargo build");
    assert_eq!(strip_safe_wrappers("time cargo test"), "cargo test");
}

#[test]
fn test_strip_combined() {
    assert_eq!(
        strip_safe_wrappers("RUST_LOG=info timeout 60 cargo test"),
        "cargo test"
    );
}

#[test]
fn test_get_command_prefix() {
    assert_eq!(get_command_prefix("git status"), Some("git status".into()));
    assert_eq!(
        get_command_prefix("git commit -m msg"),
        Some("git commit".into())
    );
    assert_eq!(
        get_command_prefix("cargo test --release"),
        Some("cargo test".into())
    );
    // Safe leading env var is stripped before extraction.
    assert_eq!(
        get_command_prefix("RUST_LOG=debug cargo test"),
        Some("cargo test".into())
    );
    // Hyphenated subcommands are valid.
    assert_eq!(
        get_command_prefix("gh pr-list --state open"),
        Some("gh pr-list".into())
    );
}

#[test]
fn test_get_command_prefix_requires_subcommand_shape() {
    // Second token is a flag, not a subcommand → exact rule (None).
    assert_eq!(get_command_prefix("ls -la"), None);
    assert_eq!(get_command_prefix("docker -v"), None);
    assert_eq!(get_command_prefix("bash -c 'evil'"), None);
    // Filename / path / number are not subcommands.
    assert_eq!(get_command_prefix("git a.txt"), None);
    assert_eq!(get_command_prefix("chmod 755 file"), None);
    assert_eq!(get_command_prefix("python /tmp/x.py"), None);
    // Single bare word → exact rule (None).
    assert_eq!(get_command_prefix("make"), None);
}

/// `get_command_prefix` has NO bare-shell guard — a wrapper/privilege command
/// with a subcommand-shaped second token yields a two-word prefix. The
/// bare-shell guard lives in `get_first_word_prefix`.
#[test]
fn test_get_command_prefix_no_bare_guard() {
    assert_eq!(get_command_prefix("sudo rm -rf /"), Some("sudo rm".into()));
    assert_eq!(
        get_command_prefix("env malicious"),
        Some("env malicious".into())
    );
    assert_eq!(
        get_command_prefix("/usr/bin/sudo apt install"),
        Some("/usr/bin/sudo apt".into())
    );
    assert_eq!(
        get_command_prefix("nohup npm start"),
        Some("nohup npm".into())
    );
}

#[test]
fn test_get_command_prefix_rejects_unsafe_env() {
    // An unsafe leading env var survives stripping → no prefix (would embed the
    // env value in the rule and could never match a different value).
    assert_eq!(
        get_command_prefix("LD_PRELOAD=/evil.so cat /etc/passwd"),
        None
    );
    assert_eq!(get_command_prefix("MY_VAR=val npm run build"), None);
}

#[test]
fn test_get_first_word_prefix() {
    // Falls back to the single command word when no subcommand prefix exists.
    assert_eq!(get_first_word_prefix("ls -la"), Some("ls".into()));
    assert_eq!(
        get_first_word_prefix("python3 script.py"),
        Some("python3".into())
    );
    assert_eq!(get_first_word_prefix("docker -v"), Some("docker".into()));
    // Safe leading env var is skipped first.
    assert_eq!(
        get_first_word_prefix("RUST_LOG=debug cargo"),
        Some("cargo".into())
    );
}

#[test]
fn test_get_first_word_prefix_rejects_bare_shell_and_bad_shapes() {
    // Bare shells / wrappers / privilege escalators are blocked here (TS).
    assert_eq!(get_first_word_prefix("bash -c 'evil'"), None);
    assert_eq!(get_first_word_prefix("sudo rm -rf /"), None);
    assert_eq!(get_first_word_prefix("env malicious"), None);
    assert_eq!(get_first_word_prefix("xargs rm"), None);
    // Paths / flags / numbers are not clean command words.
    assert_eq!(get_first_word_prefix("/usr/bin/python x"), None);
    assert_eq!(get_first_word_prefix("./script.sh"), None);
    assert_eq!(get_first_word_prefix("-x foo"), None);
    // Unsafe leading env var → None.
    assert_eq!(get_first_word_prefix("LD_PRELOAD=x cat"), None);
}

#[test]
fn test_heredoc_command_prefix() {
    // Two-word prefix extracted from the words before `<<`.
    assert_eq!(
        heredoc_command_prefix("git commit -m \"$(cat <<'EOF'\nbody\nEOF\n)\""),
        Some("git commit".into())
    );
    // Single bare command before the heredoc → that word (2-token fallback).
    assert_eq!(
        heredoc_command_prefix("cat <<EOF\nhi\nEOF"),
        Some("cat".into())
    );
    // Fallback preserves a flag (up to 2 tokens), mirroring TS.
    assert_eq!(
        heredoc_command_prefix("python3 -c <<EOF"),
        Some("python3 -c".into())
    );
    // No heredoc → None.
    assert_eq!(heredoc_command_prefix("echo hi"), None);
    // Unsafe leading env var before the heredoc → None.
    assert_eq!(heredoc_command_prefix("LD_PRELOAD=x cat <<EOF"), None);
}

#[test]
fn test_split_compound() {
    let parts = split_compound_command("git add . && git commit -m msg");
    assert_eq!(parts, vec!["git add .", "git commit -m msg"]);
}

#[test]
fn test_split_pipe() {
    let parts = split_compound_command("cat file | grep pattern");
    assert_eq!(parts, vec!["cat file", "grep pattern"]);
}

#[test]
fn test_split_semicolon() {
    let parts = split_compound_command("cd /tmp; ls");
    assert_eq!(parts, vec!["cd /tmp", "ls"]);
}

#[test]
fn test_split_preserves_quotes() {
    let parts = split_compound_command("echo 'hello && world'");
    assert_eq!(parts, vec!["echo 'hello && world'"]);
}

#[test]
fn test_analyze_compound_filters_noop_cd_current_cwd() {
    let cwd = "/Users/example/project";
    let analysis = analyze_compound_command("cd /Users/example/project && git diff", cwd);
    assert_eq!(analysis.significant_subcommands, vec!["git diff"]);
    assert_eq!(analysis.cwd_change_count, 1);
    assert_eq!(analysis.non_noop_cwd_change_count, 0);
    assert!(analysis.has_git);
}

#[test]
fn test_analyze_compound_filters_equivalent_relative_noop_cd() {
    let cwd = "/Users/example/project";
    let analysis = analyze_compound_command("cd . && git log --oneline", cwd);
    assert_eq!(analysis.significant_subcommands, vec!["git log --oneline"]);
    assert_eq!(analysis.non_noop_cwd_change_count, 0);
    assert!(analysis.has_git);
}

#[test]
fn test_analyze_compound_keeps_non_noop_cd_before_git() {
    let cwd = "/Users/example/project";
    let analysis = analyze_compound_command("cd ../other && git diff", cwd);
    assert_eq!(
        analysis.significant_subcommands,
        vec!["cd ../other", "git diff"]
    );
    assert_eq!(analysis.non_noop_cwd_change_count, 1);
    assert!(analysis.has_non_noop_cwd_change());
    assert!(analysis.has_git);
}

#[test]
fn test_analyze_compound_detects_multiple_cwd_changes() {
    let analysis = analyze_compound_command("cd a && cd b && ls", "/tmp/project");
    assert_eq!(analysis.non_noop_cwd_change_count, 2);
    assert!(analysis.has_multiple_cwd_changes());
}

#[test]
fn test_strip_all_env_vars() {
    assert_eq!(
        strip_all_env_vars("FOO=bar BAZ=qux command", /*check_hijack*/ false),
        "command"
    );
}

#[test]
fn test_strip_all_env_vars_blocks_hijack() {
    let result = strip_all_env_vars("LD_PRELOAD=/evil.so command", /*check_hijack*/ true);
    assert_eq!(result, "LD_PRELOAD=/evil.so command");
}

#[test]
fn test_is_dangerous_bare_prefix() {
    assert!(is_dangerous_bare_prefix("bash"));
    assert!(is_dangerous_bare_prefix("sudo"));
    assert!(is_dangerous_bare_prefix("env"));
    assert!(!is_dangerous_bare_prefix("git"));
    assert!(!is_dangerous_bare_prefix("cargo"));
}

#[test]
fn test_strip_output_redirections() {
    assert_eq!(strip_output_redirections("cmd"), "cmd");
    assert_eq!(
        strip_output_redirections("python s.py > out.txt"),
        "python s.py"
    );
    assert_eq!(strip_output_redirections("cmd >>log 2>&1"), "cmd");
    assert_eq!(strip_output_redirections("cmd 2>&1"), "cmd");
    assert_eq!(strip_output_redirections("cmd &> out"), "cmd");
    assert_eq!(strip_output_redirections("cmd 2> err.txt"), "cmd");
    // Redirections inside quotes are preserved.
    assert_eq!(
        strip_output_redirections("echo '> not a redir'"),
        "echo '> not a redir'"
    );
    assert_eq!(
        strip_output_redirections("echo \"a > b\""),
        "echo \"a > b\""
    );
}

#[test]
fn test_extract_output_redirect_targets_basic() {
    assert_eq!(
        extract_output_redirect_targets("echo x > out.txt"),
        vec!["out.txt".to_string()]
    );
    assert_eq!(
        extract_output_redirect_targets("echo x >> log"),
        vec!["log".to_string()]
    );
    assert_eq!(
        extract_output_redirect_targets("cmd > /etc/passwd"),
        vec!["/etc/passwd".to_string()]
    );
    // Clobber `>|` and combined `&>`.
    assert_eq!(
        extract_output_redirect_targets("cmd >| f"),
        vec!["f".to_string()]
    );
    assert_eq!(
        extract_output_redirect_targets("cmd &> all.log"),
        vec!["all.log".to_string()]
    );
}

#[test]
fn test_extract_output_redirect_targets_skips_fd_dups() {
    // `2>&1`, `>&-` are fd duplications, not file targets.
    assert!(extract_output_redirect_targets("cmd 2>&1").is_empty());
    assert!(extract_output_redirect_targets("cmd >&-").is_empty());
    // `> out 2>&1`: only the file target is collected.
    assert_eq!(
        extract_output_redirect_targets("cmd > out 2>&1"),
        vec!["out".to_string()]
    );
}

#[test]
fn test_extract_output_redirect_targets_quote_aware() {
    // A `>` inside quotes is not a redirection.
    assert!(extract_output_redirect_targets("echo '> not a redir'").is_empty());
    assert!(extract_output_redirect_targets("echo \"a > b\"").is_empty());
    assert!(extract_output_redirect_targets("git log --format='%H>%s'").is_empty());
}

#[test]
fn test_extract_output_redirect_targets_none() {
    assert!(extract_output_redirect_targets("ls -la").is_empty());
    assert!(extract_output_redirect_targets("cat a.txt | grep foo").is_empty());
}

#[test]
fn test_has_process_substitution() {
    // Input process substitution.
    assert!(has_process_substitution("diff <(sort a) <(sort b)"));
    // Redirect to output process substitution.
    assert!(has_process_substitution("echo x > >(tee out)"));
    assert!(has_process_substitution("echo x >>(tee out)"));
    // Plain redirects / commands are not process substitution.
    assert!(!has_process_substitution("echo x > out.txt"));
    assert!(!has_process_substitution("ls -la"));
    assert!(!has_process_substitution("echo (a)"));
}
