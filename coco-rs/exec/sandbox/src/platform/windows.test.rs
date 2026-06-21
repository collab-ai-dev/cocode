use super::*;

#[test]
fn test_windows_sandbox_available() {
    let sandbox = WindowsSandbox;
    // coco-rs ships no native Windows sandbox backend → always unavailable
    // (the enable gate fail-closes to unsandboxed).
    assert!(!sandbox.available());
}

#[test]
fn test_wrap_command_disabled_enforcement() {
    let sandbox = WindowsSandbox;
    let config = SandboxConfig {
        enforcement: EnforcementLevel::Disabled,
        ..Default::default()
    };
    let mut cmd = tokio::process::Command::new("echo");
    cmd.arg("hello");

    let result = sandbox.wrap_command(&config, "echo hello", "_test_SBX", &[], &mut cmd);
    assert!(result.is_ok());

    // Command should not be modified when enforcement is disabled
    let inner = cmd.as_std();
    assert_eq!(inner.get_program(), "echo");
}

#[test]
fn test_wrap_command_with_enforcement() {
    let sandbox = WindowsSandbox;
    let config = SandboxConfig {
        enforcement: EnforcementLevel::WorkspaceWrite,
        ..Default::default()
    };
    let mut cmd = tokio::process::Command::new("bash");
    cmd.arg("-c").arg("ls -la");

    let result = sandbox.wrap_command(&config, "ls -la", "_test_SBX", &[], &mut cmd);
    assert!(result.is_ok());

    // Command should be replaced with coco helper
    let inner = cmd.as_std();
    let program = inner.get_program().to_string_lossy().to_string();
    // On non-Windows, the program will be the coco binary path
    assert!(!program.is_empty());

    let args: Vec<String> = inner
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();
    assert_eq!(args[0], APPLY_WINDOWS_SANDBOX_ARG1);
    // args[1] is the base64-encoded config
    // args[2] is "--"
    assert_eq!(args[2], "--");
    // args[3] is the original program
    assert_eq!(args[3], "bash");
}
