use super::*;

#[test]
fn simple_pipe_gets_redirect() {
    let out = rearrange_pipe_command("rg foo | wc -l");
    assert!(out.contains("< /dev/null"), "got: {out}");
    assert!(out.starts_with('\''), "got: {out}");
}

#[test]
fn heredoc_skipped() {
    let out = rearrange_pipe_command("cat <<EOF\nhi\nEOF");
    assert!(!out.contains("/dev/null"), "got: {out}");
}

#[test]
fn existing_redirect_skipped() {
    let out = rearrange_pipe_command("cat < file.txt");
    assert!(!out.contains("/dev/null"), "got: {out}");
    assert!(out.starts_with('\''));
}

#[test]
fn simple_command_gets_redirect() {
    let out = rearrange_pipe_command("ls -la");
    assert!(out.contains("< /dev/null"));
}
