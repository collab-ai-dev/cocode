use crate::executor::ShellExecutor;
use crate::result::ExecOptions;

struct ReplacingSandboxPlatform;

impl coco_sandbox::SandboxPlatform for ReplacingSandboxPlatform {
    fn available(&self) -> bool {
        true
    }

    fn wrap_command(
        &self,
        _config: &coco_sandbox::SandboxConfig,
        _command: &str,
        _session_tag: &str,
        _extra_writable_binds: &[std::path::PathBuf],
        cmd: &mut tokio::process::Command,
    ) -> coco_sandbox::error::Result<()> {
        let program = cmd.as_std().get_program().to_os_string();
        let args = cmd
            .as_std()
            .get_args()
            .map(std::ffi::OsStr::to_os_string)
            .collect::<Vec<_>>();
        *cmd = tokio::process::Command::new(program);
        cmd.args(args);
        Ok(())
    }
}

#[tokio::test]
async fn test_execute_echo() {
    let mut exec = ShellExecutor::new(std::path::Path::new("/tmp"));
    let result = exec
        .execute("echo hello", &ExecOptions::default())
        .await
        .unwrap();
    assert_eq!(result.exit_code, 0);
    assert!(result.stdout.contains("hello"));
}

#[tokio::test]
async fn test_execute_exit_code() {
    let mut exec = ShellExecutor::new(std::path::Path::new("/tmp"));
    let result = exec
        .execute("exit 42", &ExecOptions::default())
        .await
        .unwrap();
    assert_eq!(result.exit_code, 42);
}

#[tokio::test]
async fn test_cwd_tracking() {
    let mut exec = ShellExecutor::new(std::path::Path::new("/tmp"));
    let result = exec
        .execute("cd /usr && pwd", &ExecOptions::default())
        .await
        .unwrap();
    assert_eq!(result.exit_code, 0);
    assert!(result.stdout.contains("/usr"));
    assert_eq!(exec.cwd(), std::path::Path::new("/usr"));
}

/// `remove_env` is applied after `extra_env`, so a name in both is stripped.
/// Unattended paths (cron script jobs) rely on this ordering to keep provider
/// credentials out of the child no matter what else set them.
#[tokio::test]
async fn test_remove_env_strips_a_name_extra_env_had_set() {
    let mut exec = ShellExecutor::new(std::path::Path::new("/tmp"));
    let opts = ExecOptions {
        extra_env: [
            ("COCO_TEST_KEPT".to_string(), "kept".to_string()),
            ("COCO_TEST_SECRET".to_string(), "leaked".to_string()),
        ]
        .into_iter()
        .collect(),
        remove_env: vec!["COCO_TEST_SECRET".to_string()],
        ..Default::default()
    };
    let result = exec
        .execute(
            "echo \"kept=${COCO_TEST_KEPT-unset} secret=${COCO_TEST_SECRET-unset}\"",
            &opts,
        )
        .await
        .unwrap();
    assert_eq!(result.exit_code, 0);
    assert!(
        result.stdout.contains("kept=kept secret=unset"),
        "got: {}",
        result.stdout
    );
}

#[tokio::test]
async fn sandbox_wrapper_cannot_discard_spawn_cwd_or_environment() {
    let cwd = tempfile::tempdir().expect("tempdir");
    let state = coco_sandbox::SandboxState::new(
        coco_sandbox::EnforcementLevel::WorkspaceWrite,
        coco_sandbox::SandboxSettings::enabled(),
        coco_sandbox::SandboxConfig {
            enforcement: coco_sandbox::EnforcementLevel::WorkspaceWrite,
            allow_network: true,
            ..Default::default()
        },
        Box::new(ReplacingSandboxPlatform),
    );
    let mut exec = ShellExecutor::new(cwd.path());
    let opts = ExecOptions {
        sandbox: Some(std::sync::Arc::new(state)),
        extra_env: [("COCO_WRAP_ORDER".to_string(), "preserved".to_string())]
            .into_iter()
            .collect(),
        ..Default::default()
    };

    let result = exec
        .execute("printf '%s\\n' \"$COCO_WRAP_ORDER\"; pwd", &opts)
        .await
        .expect("sandboxed execution");

    assert!(result.stdout.contains("preserved"));
    assert!(result.stdout.contains(&cwd.path().display().to_string()));
}

#[tokio::test]
async fn test_timeout() {
    let mut exec = ShellExecutor::new(std::path::Path::new("/tmp"));
    let opts = ExecOptions {
        timeout_ms: Some(100),
        ..Default::default()
    };
    let result = exec.execute("sleep 10", &opts).await.unwrap();
    assert!(result.timed_out);
}

#[cfg(unix)]
#[tokio::test]
async fn direct_shell_exit_reaps_descendants_holding_output_pipes() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let sentinel = dir.path().join("descendant-survived");
    let mut exec = ShellExecutor::new(dir.path());
    let opts = ExecOptions {
        timeout_ms: Some(20_000),
        extra_env: [(
            "COCO_TEST_DESCENDANT_SENTINEL".to_string(),
            sentinel.display().to_string(),
        )]
        .into_iter()
        .collect(),
        ..Default::default()
    };

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        exec.execute(
            "(sleep 0.2; printf leaked > \"$COCO_TEST_DESCENDANT_SENTINEL\") &",
            &opts,
        ),
    )
    .await
    .expect("descendant pipe must not hold execute open")
    .expect("shell execution");

    assert_eq!(result.exit_code, 0);
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    assert!(
        !sentinel.exists(),
        "a descendant survived the direct shell exit"
    );
}

#[tokio::test]
async fn test_safety_check() {
    let exec = ShellExecutor::new(std::path::Path::new("/tmp"));
    assert!(exec.check_safety("ls -la").is_safe());
    // Destructive commands are never hard-denied — they require approval,
    // surfacing the advisory note as the reason.
    let rm = exec.check_safety("rm -rf /");
    assert!(!rm.is_safe() && !rm.is_denied());
    assert!(!exec.check_safety("npm install").is_safe());
}
