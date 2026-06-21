use super::*;
use pretty_assertions::assert_eq;

#[test]
fn parse_zsh_lc_plain_commands() {
    let command = vec!["zsh".to_string(), "-lc".to_string(), "ls".to_string()];
    let parsed = parse_shell_lc_plain_commands(&command).unwrap();
    assert_eq!(parsed, vec![vec!["ls".to_string()]]);
}

#[test]
fn parse_shell_lc_single_command_prefix_supports_heredoc() {
    let command = vec![
        "zsh".to_string(),
        "-lc".to_string(),
        "python3 <<'PY'\nprint('hello')\nPY".to_string(),
    ];
    let parsed = parse_shell_lc_single_command_prefix(&command);
    assert_eq!(parsed, Some(vec!["python3".to_string()]));
}

#[test]
fn parse_shell_lc_single_command_prefix_rejects_multi_command_scripts() {
    let command = vec![
        "bash".to_string(),
        "-lc".to_string(),
        "python3 <<'PY'\nprint('hello')\nPY\necho done".to_string(),
    ];
    assert_eq!(parse_shell_lc_single_command_prefix(&command), None);
}

#[test]
fn parse_shell_lc_single_command_prefix_rejects_non_heredoc_redirects() {
    let command = vec![
        "bash".to_string(),
        "-lc".to_string(),
        "echo hello > /tmp/out.txt".to_string(),
    ];
    assert_eq!(parse_shell_lc_single_command_prefix(&command), None);
}

#[test]
fn extract_bash_command_works() {
    let cmd = vec!["bash".to_string(), "-lc".to_string(), "ls -la".to_string()];
    let (shell, script) = extract_bash_command(&cmd).unwrap();
    assert_eq!(shell, "bash");
    assert_eq!(script, "ls -la");
}

#[test]
fn extract_bash_command_rejects_invalid() {
    let cmd = vec![
        "python".to_string(),
        "-c".to_string(),
        "print(1)".to_string(),
    ];
    assert!(extract_bash_command(&cmd).is_none());
}
