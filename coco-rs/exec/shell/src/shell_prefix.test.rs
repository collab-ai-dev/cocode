use super::*;
use pretty_assertions::assert_eq;

#[test]
fn no_args() {
    assert_eq!(
        format_shell_prefix_command("bash", "ls -la"),
        "'bash' 'ls -la'"
    );
}

#[test]
fn with_args() {
    assert_eq!(
        format_shell_prefix_command("/usr/bin/bash -c", "ls -la"),
        "'/usr/bin/bash' -c 'ls -la'"
    );
}

#[test]
fn with_complex_args() {
    // rfind(' -') splits on the LAST ` -`: "tmux exec -- bash" (exec) + "-lc" (args).
    let out = format_shell_prefix_command("tmux exec -- bash -lc", "echo hi");
    assert_eq!(out, "'tmux exec -- bash' -lc 'echo hi'");
}

#[test]
fn windows_path_with_spaces() {
    assert_eq!(
        format_shell_prefix_command(r#"C:\Program Files\Git\bin\bash.exe -c"#, "ls"),
        r#"'C:\Program Files\Git\bin\bash.exe' -c 'ls'"#
    );
}

#[test]
fn escapes_single_quote_in_command() {
    let out = format_shell_prefix_command("bash", "echo 'hi'");
    assert!(out.contains(r#"'"'"'"#));
}
