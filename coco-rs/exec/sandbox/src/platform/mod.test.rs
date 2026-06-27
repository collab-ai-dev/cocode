use super::*;
use crate::config::SandboxConfig;

#[test]
fn test_create_platform_available() {
    let sandbox = create_platform();
    // On macOS or Linux, the platform sandbox should report availability
    // (Linux depends on bwrap being installed)
    if cfg!(target_os = "macos") {
        assert!(sandbox.available());
    }
    // On other platforms, it should not be available
    if cfg!(not(any(target_os = "macos", target_os = "linux"))) {
        assert!(!sandbox.available());
    }
}

#[test]
fn test_create_platform_wrap_disabled_mode() {
    let sandbox = create_platform();
    let config = SandboxConfig::default();
    let mut cmd = tokio::process::Command::new("echo");
    cmd.arg("hello");
    // Wrapping with disabled enforcement should be a no-op
    assert!(
        sandbox
            .wrap_command(&config, "echo hello", "_test_SBX", &[], &mut cmd)
            .is_ok()
    );
}
