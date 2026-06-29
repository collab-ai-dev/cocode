use crate::environment::Platform;
use crate::environment::ShellKind;
use crate::environment::build_agent_proxy_env_line;
use crate::environment::get_environment_info;

#[test]
fn test_platform_current() {
    let p = Platform::current();
    assert!(matches!(
        p,
        Platform::Linux | Platform::Darwin | Platform::Windows
    ));
}

#[test]
fn test_shell_detect() {
    let s = ShellKind::detect();
    // Should return some valid shell
    assert!(matches!(
        s,
        ShellKind::Bash | ShellKind::Zsh | ShellKind::Sh | ShellKind::PowerShell
    ));
}

#[test]
fn test_get_environment_info() {
    let info = get_environment_info(
        std::path::Path::new("/tmp"),
        "test-model",
        /*include_git_status*/ true,
    );
    assert_eq!(info.cwd, "/tmp");
    assert_eq!(info.model, "test-model");
    assert!(!info.is_git_repo);
    assert!(info.agent_proxy_env_line.is_none());
}

#[test]
fn build_agent_proxy_env_line_matches_remote_diagnostic_shape() {
    let line = build_agent_proxy_env_line("/tmp/ca.pem", None);
    assert_eq!(
        line,
        "Outbound HTTPS goes through a pre-configured agent proxy (CA bundle: /tmp/ca.pem). \
         If a tool fails TLS verification or gets 403/405/407 from the proxy, \
         run curl -sS \"$HTTPS_PROXY/__agentproxy/status\" for per-tool fixes and proxy state; \
         never disable TLS verification or unset HTTPS_PROXY."
    );

    let with_readme = build_agent_proxy_env_line("/tmp/ca.pem", Some("/tmp/README.md"));
    assert!(with_readme.contains("see /tmp/README.md and run curl -sS"));
}

#[test]
fn test_get_environment_info_git_status_gated() {
    // In a real repo, `include_git_status = false` suppresses the status
    // snapshot while still reporting `is_git_repo`. Use the repo root of this
    // crate's workspace (a `.git` exists at the repo root, not here), so assert
    // on the gate behavior directly: even when a repo IS present, the flag
    // controls whether git_status is populated.
    let cwd = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let gated = get_environment_info(cwd, "m", /*include_git_status*/ false);
    assert!(gated.git_status.is_none());
}
