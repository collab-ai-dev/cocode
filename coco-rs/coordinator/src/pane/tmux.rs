//! Tmux pane backend for teammate execution.
//!
//! Manages tmux panes for teammates — creating splits, setting border colors
//! and titles, hiding/showing panes, and rebalancing layouts.

use async_trait::async_trait;
use tokio::sync::Mutex;

use super::CreatePaneResult;
use super::PaneBackend;
use super::PaneId;
use crate::constants::AgentColorName;
use crate::constants::HIDDEN_SESSION_NAME;
use crate::constants::SWARM_SESSION_NAME;
use crate::constants::SWARM_VIEW_WINDOW_NAME;
use crate::constants::TMUX_COMMAND;
use crate::types::BackendType;

/// Benign holding process every pane is created running. A pane must start
/// with a placeholder that never exits on its own so the later
/// `respawn-pane -k` is the *only* thing that ever puts a real process in the
/// pane — replacing the racy `send-keys … Enter` + shell-init sleep, which
/// also leaked the relaunch command as visible keystrokes.
const PANE_HOLD_COMMAND: &str = "cat";

/// Tmux pane backend.
pub struct TmuxBackend {
    /// Whether we're inside tmux (leader's pane exists).
    is_native: bool,
    /// Lock for sequential pane creation (avoids race conditions).
    pane_creation_lock: Mutex<()>,
    /// Cached leader window target (used for rebalancing).
    _cached_leader_window: Mutex<Option<String>>,
    /// Whether the first pane was used for external session.
    first_pane_used: Mutex<bool>,
}

impl TmuxBackend {
    pub fn new(is_native: bool) -> Self {
        Self {
            is_native,
            pane_creation_lock: Mutex::new(()),
            _cached_leader_window: Mutex::new(None),
            first_pane_used: Mutex::new(false),
        }
    }

    /// The tmux `-L` socket for THIS backend's server, or `None` to use the
    /// inherited `$TMUX` server.
    ///
    /// - **Native** (`is_native=true`): the leader is inside tmux, so ops
    ///   address the inherited `$TMUX` server — no `-L` (`None`).
    /// - **External** (`is_native=false`): there is no inherited server, so the
    ///   backend runs a dedicated PID-scoped server (`claude-swarm-<pid>`);
    ///   EVERY op must address it with `-L` (`Some`).
    fn socket(&self) -> Option<String> {
        (!self.is_native).then(crate::constants::swarm_socket_name)
    }

    /// The single tmux entry point. Routes every command through the backend's
    /// own server ([`Self::socket`]) so native and external never diverge on
    /// which server they target — the bug that left external-session panes
    /// created on the swarm socket but killed/commanded on the default one.
    async fn run(&self, args: &[&str]) -> crate::Result<String> {
        match self.socket() {
            Some(socket) => run_tmux_with_socket(&socket, args).await,
            None => run_tmux(args).await,
        }
    }

    /// The active window target (`session:window`) for the inherited client,
    /// or `None` when nothing is attached / the query fails. Used only as the
    /// fallback target for per-window options.
    async fn current_window_target(&self) -> Option<String> {
        self.run(&["display-message", "-p", "#{session_name}:#{window_index}"])
            .await
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }
}

#[async_trait]
impl PaneBackend for TmuxBackend {
    fn backend_type(&self) -> BackendType {
        BackendType::Tmux
    }

    fn display_name(&self) -> &str {
        "tmux"
    }

    fn supports_hide_show(&self) -> bool {
        true
    }

    async fn is_available(&self) -> bool {
        super::is_tmux_available().await
    }

    async fn is_running_inside(&self) -> bool {
        self.is_native
    }

    async fn create_teammate_pane(
        &self,
        name: &str,
        color: AgentColorName,
    ) -> crate::Result<CreatePaneResult> {
        let _lock = self.pane_creation_lock.lock().await;

        let is_first = {
            let mut first = self.first_pane_used.lock().await;
            let was_first = !*first;
            *first = true;
            was_first
        };

        if self.is_native {
            self.create_teammate_pane_with_leader(name, color, is_first)
                .await
        } else {
            self.create_teammate_pane_external(name, color, is_first)
                .await
        }
    }

    async fn send_command_to_pane(&self, pane_id: &PaneId, command: &str) -> crate::Result<()> {
        super::assert_no_control_chars(command)?;
        self.run(&remain_on_exit_args(pane_id)).await?;
        self.run(&respawn_pane_args(pane_id, command)).await?;
        Ok(())
    }

    async fn set_pane_border_color(
        &self,
        pane_id: &PaneId,
        color: AgentColorName,
    ) -> crate::Result<()> {
        let tmux_color = agent_color_to_tmux(color);
        // Three-step sequence. Step 1 sets the pane's foreground colour for
        // the border; steps 2-3 set the per-pane `pane-border-style` and
        // `pane-active-border-style` options so the border keeps its colour
        // whether the pane is active or inactive (requires tmux 3.2+).
        self.run(&[
            "select-pane",
            "-t",
            pane_id,
            "-P",
            &format!("bg=default,fg={tmux_color}"),
        ])
        .await?;
        self.run(&[
            "set-option",
            "-p",
            "-t",
            pane_id,
            "pane-border-style",
            &format!("fg={tmux_color}"),
        ])
        .await?;
        self.run(&[
            "set-option",
            "-p",
            "-t",
            pane_id,
            "pane-active-border-style",
            &format!("fg={tmux_color}"),
        ])
        .await?;
        Ok(())
    }

    async fn set_pane_title(
        &self,
        pane_id: &PaneId,
        name: &str,
        _color: AgentColorName,
    ) -> crate::Result<()> {
        self.run(&["select-pane", "-t", pane_id, "-T", name])
            .await?;
        Ok(())
    }

    async fn enable_pane_border_status(&self, window_target: Option<&str>) -> crate::Result<()> {
        // Scope to the window (`-w -t <target>`), NOT the server (`-g`): a
        // global set mutates the user's unrelated tmux windows and is never
        // reverted on teardown. Fall back to the active window when no target
        // is supplied; bail if none resolves.
        let target = match window_target {
            Some(t) => t.to_string(),
            None => match self.current_window_target().await {
                Some(t) => t,
                None => return Ok(()),
            },
        };
        self.run(&[
            "set-option",
            "-w",
            "-t",
            &target,
            "pane-border-status",
            "top",
        ])
        .await?;
        Ok(())
    }

    async fn rebalance_panes(&self, window_target: &str, has_leader: bool) -> crate::Result<()> {
        if has_leader {
            self.rebalance_panes_with_leader(window_target).await
        } else {
            self.rebalance_panes_tiled(window_target).await
        }
    }

    async fn kill_pane(&self, pane_id: &PaneId) -> crate::Result<bool> {
        let output = self.run(&["kill-pane", "-t", pane_id]).await;
        Ok(output.is_ok())
    }

    async fn hide_pane(&self, pane_id: &PaneId) -> crate::Result<bool> {
        // Ensure hidden session exists
        let has_hidden = self
            .run(&["has-session", "-t", HIDDEN_SESSION_NAME])
            .await
            .is_ok();

        if !has_hidden {
            self.run(&["new-session", "-d", "-s", HIDDEN_SESSION_NAME])
                .await?;
        }

        // Move pane to hidden session
        let result = self
            .run(&[
                "join-pane",
                "-d",
                "-t",
                &format!("{HIDDEN_SESSION_NAME}:"),
                "-s",
                pane_id,
            ])
            .await;

        Ok(result.is_ok())
    }

    async fn show_pane(
        &self,
        pane_id: &PaneId,
        target_window_or_pane: &str,
    ) -> crate::Result<bool> {
        let result = self
            .run(&[
                "join-pane",
                "-d",
                "-t",
                target_window_or_pane,
                "-s",
                &format!("{HIDDEN_SESSION_NAME}:{pane_id}"),
            ])
            .await;

        Ok(result.is_ok())
    }
}

impl TmuxBackend {
    /// Create a pane when the leader is inside tmux.
    ///
    /// Layout: 30% leader (left), 70% teammates (right, tiled).
    async fn create_teammate_pane_with_leader(
        &self,
        name: &str,
        color: AgentColorName,
        is_first: bool,
    ) -> crate::Result<CreatePaneResult> {
        // `-d` keeps focus on the leader (no keystroke leak); `-- cat` runs the
        // benign holding process so the later `respawn-pane -k` deterministically
        // replaces it.
        let split_args = if is_first {
            // First teammate: horizontal split, 70% right
            vec![
                "split-window",
                "-d",
                "-h",
                "-p",
                "70",
                "-P",
                "-F",
                "#{pane_id}",
                "--",
                PANE_HOLD_COMMAND,
            ]
        } else {
            // Subsequent: vertical split in the right region
            vec![
                "split-window",
                "-d",
                "-v",
                "-P",
                "-F",
                "#{pane_id}",
                "--",
                PANE_HOLD_COMMAND,
            ]
        };

        let output = self.run(&split_args).await?;
        let pane_id = output.trim().to_string();

        // Set border color and title
        let _ = self.set_pane_border_color(&pane_id, color).await;
        let _ = self.set_pane_title(&pane_id, name, color).await;

        // Enable pane border titles
        let _ = self.enable_pane_border_status(None).await;

        Ok(CreatePaneResult {
            pane_id,
            is_first_teammate: is_first,
        })
    }

    /// Create a pane in an external swarm session.
    async fn create_teammate_pane_external(
        &self,
        name: &str,
        color: AgentColorName,
        is_first: bool,
    ) -> crate::Result<CreatePaneResult> {
        // All ops route through `self.run`, which (external mode) addresses the
        // dedicated PID-scoped swarm server — same server kill_pane /
        // send_command now target too.
        let swarm_window = format!("{SWARM_SESSION_NAME}:{SWARM_VIEW_WINDOW_NAME}");
        let pane_id = if is_first {
            // Reuse an already-running swarm session/window if present rather
            // than recreating it — an unconditional `new-session` would orphan
            // the prior session's panes: has-session → list-windows →
            // list-panes, reusing panes[0].
            let has_session = self
                .run(&["has-session", "-t", SWARM_SESSION_NAME])
                .await
                .is_ok();
            if has_session {
                let windows = self
                    .run(&[
                        "list-windows",
                        "-t",
                        SWARM_SESSION_NAME,
                        "-F",
                        "#{window_name}",
                    ])
                    .await
                    .unwrap_or_default();
                let has_view = windows.lines().any(|w| w.trim() == SWARM_VIEW_WINDOW_NAME);
                if has_view {
                    // Reuse the first pane of the existing swarm-view window.
                    let panes = self
                        .run(&["list-panes", "-t", &swarm_window, "-F", "#{pane_id}"])
                        .await
                        .unwrap_or_default();
                    match panes.lines().map(str::trim).find(|s| !s.is_empty()) {
                        Some(p) => p.to_string(),
                        None => {
                            // Window exists but has no panes (unexpected): split it.
                            let output = self
                                .run(&[
                                    "split-window",
                                    "-d",
                                    "-t",
                                    &swarm_window,
                                    "-P",
                                    "-F",
                                    "#{pane_id}",
                                    "--",
                                    PANE_HOLD_COMMAND,
                                ])
                                .await?;
                            output.trim().to_string()
                        }
                    }
                } else {
                    // Session exists but the swarm-view window does not: add it.
                    let output = self
                        .run(&[
                            "new-window",
                            "-d",
                            "-t",
                            SWARM_SESSION_NAME,
                            "-n",
                            SWARM_VIEW_WINDOW_NAME,
                            "-P",
                            "-F",
                            "#{pane_id}",
                            "--",
                            PANE_HOLD_COMMAND,
                        ])
                        .await?;
                    let _ = self.enable_pane_border_status(Some(&swarm_window)).await;
                    output.trim().to_string()
                }
            } else {
                // No swarm session yet: create it. Its INITIAL pane IS the first
                // teammate's pane — TS reuses `firstPaneId` rather than splitting.
                let output = self
                    .run(&[
                        "new-session",
                        "-d",
                        "-s",
                        SWARM_SESSION_NAME,
                        "-n",
                        SWARM_VIEW_WINDOW_NAME,
                        "-P",
                        "-F",
                        "#{pane_id}",
                        "--",
                        PANE_HOLD_COMMAND,
                    ])
                    .await?;
                // Enable per-window pane titles now that the swarm window exists.
                let _ = self.enable_pane_border_status(Some(&swarm_window)).await;
                output.trim().to_string()
            }
        } else {
            // Subsequent teammates split an existing pane in the swarm window.
            let output = self
                .run(&[
                    "split-window",
                    "-d",
                    "-t",
                    &swarm_window,
                    "-P",
                    "-F",
                    "#{pane_id}",
                    "--",
                    PANE_HOLD_COMMAND,
                ])
                .await?;
            output.trim().to_string()
        };

        // Mirror the leader path: colored border + titled border, then tile
        // the swarm window. All ops route through `self.run`, which in external
        // mode already addresses the dedicated swarm server, so no
        // socket-aware variants are needed.
        let _ = self.set_pane_border_color(&pane_id, color).await;
        let _ = self.set_pane_title(&pane_id, name, color).await;
        let _ = self.rebalance_panes_tiled(&swarm_window).await;

        Ok(CreatePaneResult {
            pane_id,
            is_first_teammate: is_first,
        })
    }

    /// Rebalance panes with leader (30% leader, 70% teammates).
    async fn rebalance_panes_with_leader(&self, window_target: &str) -> crate::Result<()> {
        self.run(&["select-layout", "-t", window_target, "main-vertical"])
            .await?;
        // Set leader pane width to 30%
        self.run(&["set-option", "-t", window_target, "main-pane-width", "30%"])
            .await?;
        Ok(())
    }

    /// Rebalance panes without leader (tiled layout).
    async fn rebalance_panes_tiled(&self, window_target: &str) -> crate::Result<()> {
        self.run(&["select-layout", "-t", window_target, "tiled"])
            .await?;
        Ok(())
    }
}

// ── Tmux Helpers ──

/// `set-option` argv that arms the pane to stay open (showing the error) if
/// the relaunched command exits non-zero — a crashed teammate leaves a visible
/// dead pane instead of vanishing. `-p` scopes the option to this pane only.
fn remain_on_exit_args(pane_id: &str) -> [&str; 6] {
    [
        "set-option",
        "-p",
        "-t",
        pane_id,
        "remain-on-exit",
        "failed",
    ]
}

/// `respawn-pane -k` argv. `-k` kills the pane's current process (the `cat`
/// holder) and exec's the command in its place — no shell prompt to race, no
/// readline buffer to leak into. The command is the shell one-liner from
/// `build_teammate_command` (`cd X && env… cmd`), so host it under a fresh
/// non-interactive `sh -c` (no rc-file, no prompt, no line editor).
fn respawn_pane_args<'a>(pane_id: &'a str, command: &'a str) -> [&'a str; 8] {
    [
        "respawn-pane",
        "-k",
        "-t",
        pane_id,
        "--",
        "sh",
        "-c",
        command,
    ]
}

/// Run a tmux command and return stdout.
async fn run_tmux(args: &[&str]) -> crate::Result<String> {
    let output = tokio::process::Command::new(TMUX_COMMAND)
        .args(args)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(crate::CoordinatorError::generic(format!(
            "tmux command failed: {stderr}"
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Run a tmux command with a specific socket.
async fn run_tmux_with_socket(socket: &str, args: &[&str]) -> crate::Result<String> {
    let output = tokio::process::Command::new(TMUX_COMMAND)
        .arg("-L")
        .arg(socket)
        .args(args)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(crate::CoordinatorError::generic(format!(
            "tmux command failed: {stderr}"
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Map AgentColorName to tmux color strings.
fn agent_color_to_tmux(color: AgentColorName) -> &'static str {
    match color {
        AgentColorName::Red => "red",
        AgentColorName::Blue => "blue",
        AgentColorName::Green => "green",
        AgentColorName::Yellow => "yellow",
        AgentColorName::Purple => "magenta",
        AgentColorName::Orange => "colour208",
        AgentColorName::Pink => "colour205",
        AgentColorName::Cyan => "cyan",
    }
}

#[cfg(test)]
#[path = "tmux.test.rs"]
mod tests;
