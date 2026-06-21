//! Integration tests for sandbox enforcement.
//!
//! Verifies that sandbox restrictions are actually enforced by running commands
//! under bwrap (Linux) or Seatbelt (macOS). Tests skip gracefully when the
//! platform doesn't support sandboxing or required binaries are missing.

use coco_sandbox::EnforcementLevel;
use coco_sandbox::SandboxConfig;
use coco_sandbox::SandboxState;
use coco_sandbox::config::SandboxSettings;

// ==========================================================================
// Linux enforcement tests (bwrap + seccomp)
// ==========================================================================

#[cfg(target_os = "linux")]
mod linux {
    use std::time::Duration;

    use coco_sandbox::EnforcementLevel;
    use coco_sandbox::SandboxConfig;
    use coco_sandbox::WritableRoot;
    use coco_sandbox::platform::create_platform;

    #[cfg(not(target_arch = "aarch64"))]
    const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
    #[cfg(target_arch = "aarch64")]
    const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

    fn enforcement_available() -> bool {
        use std::sync::OnceLock;
        static AVAILABLE: OnceLock<bool> = OnceLock::new();

        *AVAILABLE.get_or_init(|| {
            let bwrap = ["/usr/bin/bwrap", "/usr/local/bin/bwrap"]
                .iter()
                .find(|p| std::path::Path::new(p).exists());
            let Some(bwrap_path) = bwrap else {
                eprintln!("Skipping: bwrap not found");
                return false;
            };
            let result = std::process::Command::new(bwrap_path)
                .args([
                    "--ro-bind",
                    "/",
                    "/",
                    "--dev",
                    "/dev",
                    "--unshare-user",
                    "--",
                    "/bin/true",
                ])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            match result {
                Ok(s) if s.success() => true,
                _ => {
                    eprintln!("Skipping: bwrap probe failed");
                    false
                }
            }
        })
    }

    macro_rules! skip_if_unavailable {
        () => {
            if !enforcement_available() {
                return Ok(());
            }
        };
    }

    async fn run_sandboxed(command: &str, config: &SandboxConfig) -> anyhow::Result<(i32, String)> {
        let platform = create_platform();
        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.arg("-c").arg(command);
        cmd.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        platform
            .wrap_command(config, command, "_test_SBX", &[], &mut cmd)
            .map_err(|e| anyhow::anyhow!("wrap failed: {e}"))?;

        let output = tokio::time::timeout(PROBE_TIMEOUT, cmd.output())
            .await
            .map_err(|_| anyhow::anyhow!("timeout"))?
            .map_err(|e| anyhow::anyhow!("spawn: {e}"))?;

        Ok((
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).to_string(),
        ))
    }

    #[tokio::test]
    async fn test_seccomp_restricted_emitted_when_network_isolated() -> anyhow::Result<()> {
        skip_if_unavailable!();
        // `allow_network: false` forces `determine_seccomp_mode` → Restricted,
        // exercising the seccomp inner-stage emit path that every OTHER test
        // skips by setting `allow_network: true`. The rebuilt argv must carry
        // the `--apply-seccomp restricted --` inner stage that the binary's
        // `dispatch_or_continue` consumes (S1 regression guard).
        let config = SandboxConfig {
            enforcement: EnforcementLevel::ReadOnly,
            allow_network: false,
            ..Default::default()
        };
        let platform = create_platform();
        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.arg("-c").arg("echo coco-sbx-ok");
        platform
            .wrap_command(&config, "echo coco-sbx-ok", "_test_SBX", &[], &mut cmd)
            .map_err(|e| anyhow::anyhow!("wrap failed: {e}"))?;

        let args: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        // bwrap … -- <coco_exe> --apply-seccomp restricted -- /bin/sh -c …
        let sec_idx = args
            .iter()
            .position(|a| a == coco_sandbox::APPLY_SECCOMP_ARG1)
            .expect("--apply-seccomp must be emitted when network is isolated");
        assert_eq!(args[sec_idx + 1], "restricted");
        assert_eq!(args[sec_idx + 2], "--");
        assert_eq!(args[sec_idx + 3], "/bin/sh");
        Ok(())
    }

    #[tokio::test]
    async fn test_readonly_allows_read() -> anyhow::Result<()> {
        skip_if_unavailable!();
        let config = SandboxConfig {
            enforcement: EnforcementLevel::ReadOnly,
            allow_network: true,
            ..Default::default()
        };
        let (code, _) = run_sandboxed("cat /etc/hostname", &config).await?;
        assert_eq!(code, 0, "ReadOnly should allow reading /etc/hostname");
        Ok(())
    }

    #[tokio::test]
    async fn test_readonly_denies_write() -> anyhow::Result<()> {
        skip_if_unavailable!();
        let config = SandboxConfig {
            enforcement: EnforcementLevel::ReadOnly,
            allow_network: true,
            ..Default::default()
        };
        let (code, _) = run_sandboxed("touch /etc/sandbox_deny_test", &config).await?;
        assert_ne!(code, 0, "ReadOnly should deny writing to /etc");
        Ok(())
    }

    #[tokio::test]
    async fn test_workspace_write_allows_root() -> anyhow::Result<()> {
        skip_if_unavailable!();
        let tmp = tempfile::tempdir()?;
        let config = SandboxConfig {
            enforcement: EnforcementLevel::WorkspaceWrite,
            writable_roots: vec![WritableRoot::unprotected(tmp.path())],
            allow_network: true,
            ..Default::default()
        };
        let cmd = format!("echo ok > {}/test.txt", tmp.path().display());
        let (code, stderr) = run_sandboxed(&cmd, &config).await?;
        assert_eq!(
            code, 0,
            "WorkspaceWrite should allow root write. stderr: {stderr}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_workspace_write_denies_outside() -> anyhow::Result<()> {
        skip_if_unavailable!();
        let tmp = tempfile::tempdir()?;
        let config = SandboxConfig {
            enforcement: EnforcementLevel::WorkspaceWrite,
            writable_roots: vec![WritableRoot::unprotected(tmp.path())],
            allow_network: true,
            ..Default::default()
        };
        let (code, _) = run_sandboxed("touch /etc/sandbox_deny_test", &config).await?;
        assert_ne!(code, 0, "WorkspaceWrite should deny /etc write");
        Ok(())
    }

    #[tokio::test]
    async fn test_env_var_hardening() -> anyhow::Result<()> {
        skip_if_unavailable!();
        let config = SandboxConfig {
            enforcement: EnforcementLevel::ReadOnly,
            allow_network: true,
            ..Default::default()
        };
        let platform = create_platform();
        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.arg("-c").arg("echo \"LD=$LD_PRELOAD\"");
        cmd.env("LD_PRELOAD", "/evil/lib.so");
        cmd.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        platform
            .wrap_command(&config, "echo", "_test_SBX", &[], &mut cmd)
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        let output = tokio::time::timeout(PROBE_TIMEOUT, cmd.output())
            .await
            .map_err(|_| anyhow::anyhow!("timeout"))?
            .map_err(|e| anyhow::anyhow!("spawn: {e}"))?;

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        assert_eq!(stdout, "LD=", "LD_PRELOAD should be cleared, got: {stdout}");
        Ok(())
    }
}

// ==========================================================================
// Cross-platform state tests
// ==========================================================================

#[test]
fn test_external_sandbox_state() {
    let settings = SandboxSettings::enabled();
    let config = SandboxConfig {
        enforcement: EnforcementLevel::WorkspaceWrite,
        ..Default::default()
    };
    let state = SandboxState::external(EnforcementLevel::WorkspaceWrite, settings, config);
    assert!(state.is_active());
    assert!(state.is_external_sandbox());
}
