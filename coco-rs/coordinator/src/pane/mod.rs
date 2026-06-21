//! Backend traits and detection for teammate execution.
//!
//! Three execution modes:
//! - **tmux**: Terminal panes visible to leader, supports hide/show
//! - **iTerm2**: Native split panes with automatic layout
//! - **in-process**: Same process, isolated via `tokio::task_local!`

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::mailbox;
use crate::types::BackendType;
use coco_types::AgentColorName;

// ── Pane Types ──

/// Pane identifier (opaque string).
pub type PaneId = String;

/// Result from creating a teammate pane.
#[derive(Debug, Clone)]
pub struct CreatePaneResult {
    pub pane_id: PaneId,
    pub is_first_teammate: bool,
}

/// Result from backend detection.
#[derive(Debug, Clone)]
pub struct BackendDetectionResult {
    pub backend_type: BackendType,
    /// Whether we're natively inside this backend (vs. launching external).
    pub is_native: bool,
    /// iTerm2-specific: needs Python API setup.
    pub needs_it2_setup: bool,
}

// ── PaneBackend Trait ──

/// Interface for pane management backends (tmux, iTerm2).
#[async_trait]
pub trait PaneBackend: Send + Sync {
    fn backend_type(&self) -> BackendType;
    fn display_name(&self) -> &str;
    fn supports_hide_show(&self) -> bool;

    async fn is_available(&self) -> bool;
    async fn is_running_inside(&self) -> bool;

    async fn create_teammate_pane(
        &self,
        name: &str,
        color: AgentColorName,
    ) -> crate::Result<CreatePaneResult>;

    async fn send_command_to_pane(&self, pane_id: &PaneId, command: &str) -> crate::Result<()>;

    async fn set_pane_border_color(
        &self,
        pane_id: &PaneId,
        color: AgentColorName,
    ) -> crate::Result<()>;

    async fn set_pane_title(
        &self,
        pane_id: &PaneId,
        name: &str,
        color: AgentColorName,
    ) -> crate::Result<()>;

    /// Enable pane border status display (shows pane titles).
    async fn enable_pane_border_status(&self, window_target: Option<&str>) -> crate::Result<()>;

    async fn rebalance_panes(&self, window_target: &str, has_leader: bool) -> crate::Result<()>;

    async fn kill_pane(&self, pane_id: &PaneId) -> crate::Result<bool>;

    async fn hide_pane(&self, pane_id: &PaneId) -> crate::Result<bool>;

    async fn show_pane(&self, pane_id: &PaneId, target_window_or_pane: &str)
    -> crate::Result<bool>;
}

// ── TeammateExecutor Trait ──

/// Configuration for spawning a teammate.
#[derive(Debug, Clone, Default)]
pub struct TeammateSpawnConfig {
    pub name: String,
    pub team_name: String,
    pub color: Option<AgentColorName>,
    pub plan_mode_required: bool,
    pub prompt: String,
    pub cwd: String,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub system_prompt_mode: SystemPromptMode,
    pub worktree_path: Option<String>,
    pub parent_session_id: String,
    pub permissions: Vec<String>,
    pub allow_permission_prompts: bool,
    /// Reasoning-effort override (TS `AgentTool.tsx` `effort` input).
    pub effort: Option<coco_types::ReasoningEffort>,
    /// Cache-identical tool schema reuse flag (TS `runAgent.ts:624`).
    pub use_exact_tools: bool,
    /// Per-agent MCP server allow-list (TS `AgentTool.tsx:206`).
    pub mcp_servers: Vec<String>,
    /// Per-agent tool deny-list (TS `agentToolUtils.ts:122-160`).
    pub disallowed_tools: Vec<String>,
    /// Hard cap on agent turns (TS `runAgent.ts:624`).
    pub max_turns: Option<i32>,
}

/// System prompt assembly mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SystemPromptMode {
    /// System prompt + teammate addendum.
    #[default]
    Default,
    /// Use only the provided system prompt.
    Replace,
    /// System prompt + addendum + provided prompt.
    Append,
}

/// Result from spawning a teammate.
#[derive(Debug)]
pub struct TeammateSpawnResult {
    pub success: bool,
    pub agent_id: String,
    pub error: Option<String>,
    /// Task ID (in-process only).
    pub task_id: Option<String>,
    /// Pane ID (pane-based only).
    pub pane_id: Option<PaneId>,
}

/// Common interface for teammate execution.
#[async_trait]
pub trait TeammateExecutor: Send + Sync {
    fn backend_type(&self) -> BackendType;

    async fn is_available(&self) -> bool;

    async fn spawn(&self, config: TeammateSpawnConfig) -> TeammateSpawnResult;

    async fn send_message(
        &self,
        agent_id: &str,
        message: mailbox::TeammateMessage,
    ) -> crate::Result<()>;

    async fn terminate(&self, agent_id: &str, reason: Option<&str>) -> crate::Result<bool>;

    async fn kill(&self, agent_id: &str) -> crate::Result<bool>;

    async fn is_active(&self, agent_id: &str) -> bool;
}

// `InProcessBackend` lives at [`crate::inprocess_backend`] (separate
// module because it wraps the concrete [`crate::runner::InProcessAgentRunner`]
// — keeping it out of `pane` keeps this module focused on the trait
// surface + the tmux/iTerm2 backends). It implements
// [`TeammateExecutor`] from this module via the public trait export.

// ── Backend Registry ──

/// Backend registry — detects, caches, and provides backends.
pub struct BackendRegistry {
    detection_result: RwLock<Option<BackendDetectionResult>>,
    in_process_backend: RwLock<Option<Arc<dyn TeammateExecutor>>>,
    pane_backend: RwLock<Option<Arc<dyn PaneBackend>>>,
    in_process_fallback_active: RwLock<bool>,
    startup_env: StartupPaneEnv,
}

#[derive(Debug, Clone)]
struct StartupPaneEnv {
    tmux: Option<String>,
    tmux_pane: Option<String>,
    term_program: Option<String>,
    iterm_session_id: Option<String>,
}

impl StartupPaneEnv {
    fn capture() -> Self {
        Self {
            tmux: coco_config::env::env_opt(coco_config::EnvKey::Tmux),
            tmux_pane: coco_config::env::env_opt(coco_config::EnvKey::TmuxPane),
            term_program: coco_config::env::env_opt(coco_config::EnvKey::TermProgram),
            iterm_session_id: coco_config::env::env_opt(coco_config::EnvKey::ItermSessionId),
        }
    }
}

impl BackendRegistry {
    pub fn new() -> Self {
        Self {
            detection_result: RwLock::new(None),
            in_process_backend: RwLock::new(None),
            pane_backend: RwLock::new(None),
            in_process_fallback_active: RwLock::new(false),
            startup_env: StartupPaneEnv::capture(),
        }
    }

    /// Detect the best available pane backend.
    ///
    /// Priority:
    /// 1. Inside tmux → use tmux
    /// 2. iTerm2 + it2 CLI → use iTerm2
    /// 3. tmux available → use tmux (external session)
    /// 4. Otherwise → in-process fallback
    pub async fn detect_backend(&self) -> BackendDetectionResult {
        {
            let cached = self.detection_result.read().await;
            if let Some(result) = cached.as_ref() {
                return result.clone();
            }
        }

        let result = detect_backend_impl(&self.startup_env).await;
        *self.detection_result.write().await = Some(result.clone());
        result
    }

    /// Register a pane backend.
    pub async fn register_pane_backend(&self, backend: Arc<dyn PaneBackend>) {
        *self.pane_backend.write().await = Some(backend);
    }

    /// Register an in-process backend.
    pub async fn register_in_process_backend(&self, backend: Arc<dyn TeammateExecutor>) {
        *self.in_process_backend.write().await = Some(backend);
    }

    /// Mark in-process fallback as active (no pane backend available).
    pub async fn mark_in_process_fallback(&self) {
        *self.in_process_fallback_active.write().await = true;
    }

    /// Check if in-process mode is active.
    pub async fn is_in_process_enabled(&self) -> bool {
        *self.in_process_fallback_active.read().await
    }

    /// Get the cached pane backend.
    pub async fn get_pane_backend(&self) -> Option<Arc<dyn PaneBackend>> {
        self.pane_backend.read().await.clone()
    }

    /// Get the in-process backend.
    pub async fn get_in_process_backend(&self) -> Option<Arc<dyn TeammateExecutor>> {
        self.in_process_backend.read().await.clone()
    }

    /// Get the appropriate teammate executor.
    pub async fn get_teammate_executor(
        &self,
        prefer_in_process: bool,
    ) -> Option<Arc<dyn TeammateExecutor>> {
        if prefer_in_process || *self.in_process_fallback_active.read().await {
            return self.in_process_backend.read().await.clone();
        }
        // Pane backends are not TeammateExecutor directly;
        // they need PaneBackendExecutor wrapping.
        // For now, fall back to in-process.
        self.in_process_backend.read().await.clone()
    }

    /// Select a teammate executor using the resolved AgentTeams mode.
    ///
    /// - `in-process` always returns the in-process executor.
    /// - explicit `tmux` / `iterm2` fail loudly when that backend is
    ///   unavailable or not registered.
    /// - `auto` prefers the detected pane backend, then falls back to
    ///   in-process when no pane executor exists.
    pub async fn select_teammate_executor(
        &self,
        mode: coco_config::TeammateMode,
        is_non_interactive: bool,
    ) -> Result<Arc<dyn TeammateExecutor>, String> {
        let in_process = || async {
            self.in_process_backend
                .read()
                .await
                .clone()
                .ok_or_else(|| "in-process teammate executor is not registered".to_string())
        };

        if is_non_interactive {
            return in_process().await;
        }

        match mode {
            coco_config::TeammateMode::InProcess => in_process().await,
            coco_config::TeammateMode::Tmux => {
                let expected = BackendType::Tmux;
                let pane = self.pane_backend.read().await.clone().ok_or_else(|| {
                    format!("{} teammate backend is not registered", mode.as_str())
                })?;
                if pane.backend_type() != expected || !pane.is_available().await {
                    return Err(format!("{} teammate backend is unavailable", mode.as_str()));
                }
                Ok(Arc::new(
                    crate::pane::pane_executor::PaneBackendExecutor::new(pane),
                ))
            }
            coco_config::TeammateMode::Auto => {
                let detected = self.detect_backend().await;
                if detected.backend_type.is_pane_backend()
                    && let Some(pane) = self.pane_backend.read().await.clone()
                    && pane.backend_type() == detected.backend_type
                    && pane.is_available().await
                {
                    return Ok(Arc::new(
                        crate::pane::pane_executor::PaneBackendExecutor::new(pane),
                    ));
                }
                self.mark_in_process_fallback().await;
                in_process().await
            }
        }
    }

    /// Get the resolved teammate mode string.
    pub async fn get_resolved_teammate_mode(&self) -> &'static str {
        if *self.in_process_fallback_active.read().await {
            "in-process"
        } else {
            "tmux"
        }
    }

    /// Reset all caches (for testing).
    pub async fn reset(&self) {
        *self.detection_result.write().await = None;
        *self.in_process_backend.write().await = None;
        *self.pane_backend.write().await = None;
        *self.in_process_fallback_active.write().await = false;
    }
}

impl Default for BackendRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Detection Logic ──

/// Check if running inside tmux.
///
/// Checks TMUX env var captured at load.
pub fn is_inside_tmux() -> bool {
    StartupPaneEnv::capture().is_inside_tmux()
}

/// Get the leader's tmux pane ID.
///
/// Returns the TMUX_PANE env var.
pub fn get_leader_pane_id() -> Option<String> {
    StartupPaneEnv::capture().tmux_pane
}

/// Check if tmux is available on the system.
pub async fn is_tmux_available() -> bool {
    tokio::process::Command::new("tmux")
        .arg("-V")
        .output()
        .await
        .is_ok_and(|o| o.status.success())
}

/// Check if running inside iTerm2.
pub fn is_in_iterm2() -> bool {
    StartupPaneEnv::capture().is_in_iterm2()
}

/// Check if the it2 CLI is available.
pub async fn is_it2_cli_available() -> bool {
    tokio::process::Command::new("it2")
        .arg("session")
        .arg("list")
        .output()
        .await
        .is_ok_and(|o| o.status.success())
}

/// Run backend detection.
impl StartupPaneEnv {
    fn is_inside_tmux(&self) -> bool {
        self.tmux.as_deref().is_some_and(|v| !v.is_empty())
    }

    fn is_in_iterm2(&self) -> bool {
        self.term_program.as_deref() == Some("iTerm.app") || self.iterm_session_id.is_some()
    }
}

async fn detect_backend_impl(startup_env: &StartupPaneEnv) -> BackendDetectionResult {
    // Priority 1: Inside tmux
    if startup_env.is_inside_tmux() {
        return BackendDetectionResult {
            backend_type: BackendType::Tmux,
            is_native: true,
            needs_it2_setup: false,
        };
    }

    // Priority 2: iTerm2 with it2 CLI
    if startup_env.is_in_iterm2() && is_it2_cli_available().await {
        return BackendDetectionResult {
            backend_type: BackendType::Iterm2,
            is_native: true,
            needs_it2_setup: false,
        };
    }

    // Priority 3: iTerm2 without it2, but tmux available
    if startup_env.is_in_iterm2() && is_tmux_available().await {
        return BackendDetectionResult {
            backend_type: BackendType::Tmux,
            is_native: false,
            needs_it2_setup: true,
        };
    }

    // Priority 4: tmux available (external session)
    if is_tmux_available().await {
        return BackendDetectionResult {
            backend_type: BackendType::Tmux,
            is_native: false,
            needs_it2_setup: false,
        };
    }

    // Fallback: in-process
    BackendDetectionResult {
        backend_type: BackendType::InProcess,
        is_native: true,
        needs_it2_setup: false,
    }
}

pub mod it2_setup;
pub mod iterm2;
pub mod layout;
pub mod pane_executor;
pub mod tmux;

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
