use crate::executor::ShellExecutor;
use crate::result::ExecOptions;

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
