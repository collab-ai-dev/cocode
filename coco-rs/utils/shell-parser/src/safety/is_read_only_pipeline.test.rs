use super::is_read_only_pipeline;

#[test]
fn allows_single_read_only_command() {
    assert!(is_read_only_pipeline("git status"));
    assert!(is_read_only_pipeline("ls -la"));
}

#[test]
fn allows_safe_pipeline() {
    assert!(is_read_only_pipeline("git log --oneline | head -10"));
}

#[test]
fn rejects_empty() {
    assert!(!is_read_only_pipeline(""));
    assert!(!is_read_only_pipeline("   "));
}

#[test]
fn rejects_mutating_command() {
    assert!(!is_read_only_pipeline("rm -rf /"));
}

#[test]
fn rejects_redirection() {
    assert!(!is_read_only_pipeline("echo bad > /etc/passwd"));
}

#[test]
fn rejects_pipe_into_mutating_stage() {
    assert!(!is_read_only_pipeline("cat x | tee /etc/passwd"));
}
