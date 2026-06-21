use super::*;
use crate::shell_types::default_user_shell;
use std::path::PathBuf;

fn provider() -> BashProvider {
    BashProvider::from_shell(default_user_shell())
}

#[tokio::test]
async fn build_exec_no_snapshot() {
    let p = provider();
    let opts = BuildExecOpts {
        id: 1,
        ..Default::default()
    };
    let built = p.build_exec_command("ls -la", &opts).await;
    assert!(built.command_string.contains("eval"));
    assert!(built.command_string.contains("pwd -P"));
    assert!(built.command_string.contains("/dev/null"));
    // PID-prefixed to avoid cross-process collisions in /tmp.
    let cwd_str = built.cwd_file_path.to_string_lossy();
    let expected_suffix = format!("coco-{}-1-cwd", std::process::id());
    assert!(
        cwd_str.contains(&expected_suffix),
        "cwd_file_path={cwd_str:?} did not contain {expected_suffix:?}"
    );
}

#[tokio::test]
async fn spawn_args_uses_login_when_no_snapshot() {
    let p = provider();
    let args = p.spawn_args("dummy");
    // No snapshot wired in tests — should fall back to -l.
    assert_eq!(args[0], "-c");
    assert_eq!(args[1], "-l");
}

#[tokio::test]
async fn build_exec_sandbox_uses_sandbox_tmpdir_for_cwd_file() {
    let tmp = tempfile::tempdir().unwrap();
    let p = provider();
    let opts = BuildExecOpts {
        id: 42,
        sandbox_tmp_dir: Some(tmp.path().to_path_buf()),
        use_sandbox: true,
    };
    let built = p.build_exec_command("echo hi", &opts).await;
    assert!(built.cwd_file_path.starts_with(tmp.path()));
    assert!(built.cwd_file_path.to_string_lossy().contains("cwd-42"));
}

#[tokio::test]
async fn env_overrides_includes_sandbox_tmpdir() {
    let p = provider();
    let opts = BuildExecOpts {
        id: 1,
        sandbox_tmp_dir: Some(PathBuf::from("/tmp/sbx-abc")),
        use_sandbox: true,
    };
    let env = p.env_overrides("echo hi", &opts).await;
    assert_eq!(env.get("TMPDIR").map(String::as_str), Some("/tmp/sbx-abc"));
    assert_eq!(
        env.get("COCO_TMPDIR").map(String::as_str),
        Some("/tmp/sbx-abc")
    );
    assert_eq!(
        env.get("TMPPREFIX").map(String::as_str),
        Some("/tmp/sbx-abc/zsh")
    );
}

#[tokio::test]
async fn env_overrides_no_sandbox_no_tmpdir() {
    let p = provider();
    let opts = BuildExecOpts {
        id: 1,
        ..Default::default()
    };
    let env = p.env_overrides("echo hi", &opts).await;
    assert!(
        env.is_empty(),
        "env should be empty without sandbox: {env:?}"
    );
}

#[tokio::test]
async fn build_exec_with_shell_prefix() {
    let shell = default_user_shell();
    let p = BashProvider::new(
        shell,
        None,
        SessionEnvVars::new(),
        Some("/usr/bin/tmux exec --".to_string()),
    );
    let opts = BuildExecOpts {
        id: 1,
        ..Default::default()
    };
    let built = p.build_exec_command("echo hi", &opts).await;
    // The prefix executable should land at the start (after single-quote).
    assert!(
        built.command_string.starts_with("'/usr/bin/tmux exec'"),
        "got: {}",
        built.command_string
    );
}

#[tokio::test]
async fn session_env_vars_flow_into_env_overrides() {
    let shell = default_user_shell();
    let vars = SessionEnvVars::new();
    vars.set("MY_VAR", "value");
    let p = BashProvider::new(shell, None, vars, None);
    let env = p.env_overrides("echo hi", &BuildExecOpts::default()).await;
    assert_eq!(env.get("MY_VAR").map(String::as_str), Some("value"));
}

#[tokio::test]
async fn rewrite_windows_null_redirect_applied() {
    let p = provider();
    let built = p
        .build_exec_command("ls 2>nul", &BuildExecOpts::default())
        .await;
    assert!(built.command_string.contains("2>/dev/null"));
    assert!(!built.command_string.contains("2>nul"));
}
