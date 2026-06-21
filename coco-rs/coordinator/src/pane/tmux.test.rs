use super::*;

/// Whether the real-tmux E2E can run here. tmux is unix-only, so on non-unix
/// platforms (Windows) this is a compile-time `false` and the probe never
/// spawns a process; on unix it checks the `tmux` binary is present and
/// functional (`tmux -V`). Lets the lifecycle test auto-skip instead of being
/// unconditionally `#[ignore]`d.
#[cfg(unix)]
async fn tmux_e2e_supported() -> bool {
    crate::pane::is_tmux_available().await
}

#[cfg(not(unix))]
#[allow(clippy::unused_async)] // async to match the unix signature (both awaited)
async fn tmux_e2e_supported() -> bool {
    false
}

#[test]
fn test_agent_color_to_tmux() {
    assert_eq!(agent_color_to_tmux(AgentColorName::Red), "red");
    assert_eq!(agent_color_to_tmux(AgentColorName::Purple), "magenta");
    assert_eq!(agent_color_to_tmux(AgentColorName::Orange), "colour208");
    assert_eq!(agent_color_to_tmux(AgentColorName::Pink), "colour205");
    assert_eq!(agent_color_to_tmux(AgentColorName::Cyan), "cyan");
}

#[test]
fn test_tmux_backend_properties() {
    let backend = TmuxBackend::new(/*is_native*/ true);
    assert_eq!(backend.backend_type(), BackendType::Tmux);
    assert_eq!(backend.display_name(), "tmux");
    assert!(backend.supports_hide_show());
}

#[test]
fn test_tmux_backend_external_properties() {
    let backend = TmuxBackend::new(/*is_native*/ false);
    assert_eq!(backend.backend_type(), BackendType::Tmux);
    assert!(!backend.is_native);
}

#[test]
fn test_pane_shell_init_delay() {
    assert_eq!(PANE_SHELL_INIT_DELAY_MS, 200);
}

/// L3 of the agent-teams E2E plan (`docs/coco-rs/agentteam-e2e-test-design.md`):
/// validate the full external-mode tmux pane lifecycle against REAL tmux —
/// `create_teammate_pane` (gap-1 spawn) AND `kill_pane` (gap-6 teardown), both
/// of which must target the SAME server. Runs on the PID-scoped swarm socket
/// (`claude-swarm-<pid>`), so it can never touch a real coco swarm (different
/// process → different socket → fully isolated, no clobber hazard).
///
/// This is also the regression test for the external-mode socket fix: before
/// it, `kill_pane` issued `tmux` with no `-L` (default socket) while the pane
/// lived on the PID socket, so the kill silently missed and the pane leaked.
/// Now both route through `TmuxBackend::run` (socket-aware).
///
/// **Self-gating — NOT `#[ignore]`d.** It runs as part of the normal suite
/// wherever a working tmux exists (Linux/macOS dev + CI with tmux), and
/// auto-skips otherwise (Windows, or no tmux installed) via
/// [`tmux_e2e_supported`]. Real coverage where possible, zero friction where
/// not. The PID-scoped socket keeps concurrent nextest processes isolated.
#[tokio::test]
async fn tmux_pane_lifecycle_create_and_kill() {
    use crate::pane::PaneBackend;

    if !tmux_e2e_supported().await {
        eprintln!("skipping tmux_pane_lifecycle_create_and_kill: no working tmux on this platform");
        return;
    }
    let socket = crate::constants::swarm_socket_name();
    // Clean slate on our private PID socket (best-effort).
    let _ = run_tmux_with_socket(&socket, &["kill-server"]).await;

    let backend = TmuxBackend::new(false); // external mode

    // Create.
    let pane = backend
        .create_teammate_pane("worker", AgentColorName::Blue)
        .await
        .expect("create_teammate_pane should create a real pane");
    let before = run_tmux_with_socket(&socket, &["list-panes", "-a", "-F", "#{pane_id}"])
        .await
        .unwrap_or_default();
    let present_before = before.lines().any(|l| l.trim() == pane.pane_id);

    // Kill (must hit the SAME server the pane was created on — the fix).
    let killed = backend
        .kill_pane(&pane.pane_id)
        .await
        .expect("kill_pane should run");
    let after = run_tmux_with_socket(&socket, &["list-panes", "-a", "-F", "#{pane_id}"])
        .await
        .unwrap_or_default();
    let present_after = after.lines().any(|l| l.trim() == pane.pane_id);

    // Teardown the whole test server before asserting (so a failed assert can't
    // leak it).
    let _ = run_tmux_with_socket(&socket, &["kill-server"]).await;

    assert!(
        present_before,
        "created pane {} not on the swarm socket; before:\n{before}",
        pane.pane_id
    );
    assert!(
        pane.is_first_teammate,
        "first create must flag is_first_teammate"
    );
    assert!(killed, "kill_pane should report success");
    assert!(
        !present_after,
        "kill_pane must remove the pane on the swarm socket (external-mode \
         socket fix); after:\n{after}"
    );
}
