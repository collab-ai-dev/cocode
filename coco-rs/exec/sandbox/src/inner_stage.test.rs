use super::*;

fn argv(parts: &[&str]) -> Vec<OsString> {
    parts.iter().map(OsString::from).collect()
}

#[test]
fn test_dispatch_or_continue_returns_on_normal_argv() {
    // argv[1] is `--prompt`, not a magic flag → returns without exec/exit.
    dispatch_or_continue(argv(&["coco", "--prompt", "hi"]));
}

#[test]
fn test_dispatch_or_continue_returns_on_empty_argv() {
    // No argv[1] → the `let Some(flag) = ... else { return }` branch.
    dispatch_or_continue(std::iter::once(OsString::from("coco")));
}

#[test]
fn test_constants_match_emit_side_literals() {
    // Guard against drift between the dispatcher match and the emit side.
    assert_eq!(APPLY_SECCOMP_ARG1, "--apply-seccomp");
    assert_eq!(APPLY_WINDOWS_SANDBOX_ARG1, "--apply-windows-sandbox");
}
