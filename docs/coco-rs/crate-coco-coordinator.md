# coco-coordinator — Crate Plan

Directory: `coordinator/` (v2)
TS source: `coordinator/coordinatorMode.ts`, `utils/swarm/` (22 files, ~7K LOC)

## Dependencies

```
coco-coordinator depends on:
  - coco-types    (AgentId, TaskId, PermissionDecision, Message)
  - coco-config   (model selection, CLI flag propagation)
  - coco-permissions (permission resolution for workers)
  - coco-tool     (ToolRegistry — worker tool filtering)
  - coco-error
  - tokio         (async runtime, channels, tasks)

coco-coordinator does NOT depend on:
  - coco-tui      (no UI — communicates via LoopEvent/channels)
  - coco-query    (workers run their own query loops)
  - coco-inference (workers hold their own ApiClient)
```

## Data Definitions

```rust
/// Coordinator mode detection and system prompt generation.
pub struct CoordinatorMode {
    pub enabled: bool,
    pub scratchpad_dir: Option<PathBuf>,
}

/// Team configuration persisted at ~/.coco/teams/{name}/config.json
pub struct TeamFile {
    pub name: String,
    pub lead_agent_id: AgentId,
    pub lead_session_id: Option<SessionId>,
    pub members: Vec<TeamMember>,
    pub team_allowed_paths: Vec<TeamAllowedPath>,
    pub hidden_pane_ids: Vec<String>,
    pub created_at: String,
}

pub struct TeamMember {
    pub agent_id: AgentId,
    pub name: String,
    pub agent_type: AgentTypeId,
    pub model: Option<String>,
    pub prompt: Option<String>,
    pub color: AgentColor,
    pub joined_at: String,
    pub cwd: PathBuf,
    pub worktree_path: Option<PathBuf>,
    pub session_id: Option<SessionId>,
    pub backend_type: BackendType,
    pub is_active: bool,
}

pub struct TeamAllowedPath {
    pub path: PathBuf,
    pub tool_id: ToolId,
    pub added_by: String,
    pub added_at: String,
}

#[derive(Clone, Copy)]
pub enum BackendType {
    InProcess,
    Tmux,
    ITerm2,
}

#[derive(Clone, Copy)]
pub enum AgentColor {
    Red, Green, Yellow, Blue, Magenta, Cyan, Orange, Purple,
}

/// Permission request between workers and leader via file-based mailbox.
pub struct SwarmPermissionRequest {
    pub id: String,
    pub worker_id: AgentId,
    pub tool_id: ToolId,
    pub input: Value,
    pub status: PermissionRequestStatus,
    pub feedback: Option<String>,
    pub created_at: String,
}

pub enum PermissionRequestStatus {
    Pending,
    Approved,
    Rejected,
}

pub struct PermissionResolution {
    pub decision: PermissionDecision,
    pub resolved_by: String,
    pub feedback: Option<String>,
    pub updated_input: Option<Value>,
    pub permission_updates: Vec<PermissionUpdate>,
}
```

## Core Logic

### Coordinator Mode (from `coordinatorMode.ts`, 369 LOC)

```rust
/// Check if running in coordinator mode (feature flag + env var).
pub fn is_coordinator_mode() -> bool;

/// Generate coordinator system prompt describing worker tools and workflow.
pub fn get_coordinator_system_prompt(
    mcp_tools: &[String],
    scratchpad_dir: Option<&Path>,
) -> String;

/// Reconcile resumed session mode with current env state.
pub fn match_session_mode(session_mode: Option<&str>) -> bool;
```

### Team Management (from `teamHelpers.ts`, 683 LOC)

```rust
/// Team file CRUD at ~/.coco/teams/{name}/config.json
pub struct TeamManager;

impl TeamManager {
    pub fn sanitize_name(name: &str) -> String;
    pub fn get_team_dir(team_name: &str) -> PathBuf;
    pub async fn read_team_file(team_name: &str) -> Option<TeamFile>;
    pub async fn write_team_file(team_name: &str, file: &TeamFile) -> Result<()>;
    pub async fn add_teammate(team_name: &str, member: TeamMember) -> Result<()>;
    pub async fn remove_teammate(team_name: &str, agent_id: &AgentId) -> Result<()>;
    pub async fn set_member_active(team_name: &str, name: &str, active: bool) -> Result<()>;
}
```

### Permission Sync (from `permissionSync.ts`, 928 LOC)

```rust
/// File-based mailbox for permission coordination.
/// Directory: ~/.coco/teams/{name}/permissions/{pending,resolved}/
pub struct PermissionMailbox;

impl PermissionMailbox {
    pub fn create_request(params: PermissionRequestParams) -> SwarmPermissionRequest;
    pub async fn send_request(request: &SwarmPermissionRequest) -> Result<()>;
    pub async fn get_pending_requests(team_name: &str) -> Vec<SwarmPermissionRequest>;
    pub async fn resolve_request(
        team_name: &str,
        request_id: &str,
        resolution: PermissionResolution,
    ) -> Result<()>;
    pub async fn poll_for_response(
        agent_name: &str,
        team_name: &str,
    ) -> Option<PermissionResolution>;
}
```

### Backend Abstraction (from `backends/`, ~2.4K LOC)

```rust
/// Pluggable execution backends for teammates.
pub trait TeammateExecutor: Send + Sync {
    fn backend_type(&self) -> BackendType;
    async fn is_available(&self) -> bool;
    async fn spawn(&self, config: TeammateSpawnConfig) -> Result<TeammateSpawnResult>;
    async fn send_message(&self, agent_id: &AgentId, message: &str) -> Result<()>;
    async fn terminate(&self, agent_id: &AgentId) -> Result<()>;
    async fn is_active(&self, agent_id: &AgentId) -> bool;
}

/// Pluggable pane backends for visual layout.
pub trait PaneBackend: Send + Sync {
    fn backend_type(&self) -> BackendType;
    async fn create_pane(&self, name: &str, color: AgentColor) -> Result<String>;
    async fn send_command_to_pane(&self, pane_id: &str, command: &str) -> Result<()>;
    async fn kill_pane(&self, pane_id: &str) -> Result<()>;
    async fn rebalance_panes(&self, has_leader: bool) -> Result<()>;
}

/// Auto-detect available backend: iTerm2 > tmux > in-process.
pub async fn detect_backend() -> BackendType;
```

### In-Process Runner (from `inProcessRunner.ts`, 1552 LOC)

```rust
/// Execute teammate in same process with context isolation.
pub async fn run_in_process_teammate(
    config: InProcessSpawnConfig,
    cancel_token: CancellationToken,
) -> Result<TeammateResult>;
```

Key behaviors:
- Separate CancellationToken per teammate (no cross-agent cancellation)
- Permission fallback: leader UI queue -> mailbox polling (500ms interval)
- Idle notification to leader on completion via mailbox
- CLI flag + env var propagation to spawned teammates

### CLI Flag Propagation (from `spawnUtils.ts`)

```rust
/// Flags propagated from leader to spawned teammates:
/// - --dangerously-skip-permissions (if bypass mode active)
/// - --permission-mode acceptEdits (alternative permission mode)
/// - --model (main loop model override, if set via CLI)
/// - --settings (settings file path, if set via CLI)
/// - --plugin-dir (for each inline plugin)
/// - --teammate-mode (so tmux teammates match leader mode)
/// - --chrome / --no-chrome (if explicitly set on CLI)
///
/// Safety: when planModeRequired=true, bypass permission flags are NOT inherited.
pub fn build_inherited_cli_flags(leader_config: &CliConfig) -> Vec<String>;

/// matchSessionMode(): auto-flips CLAUDE_CODE_COORDINATOR_MODE env var on session resume
/// to match the leader's coordinator mode setting.
pub fn match_session_mode(env: &mut HashMap<String, String>, is_coordinator: bool);
```

## Module Layout

```
coordinator/
  mod.rs                    — pub mod, re-exports
  coordinator_mode.rs       — feature detection, system prompt
  team_manager.rs           — TeamFile CRUD
  permission_mailbox.rs     — file-based permission sync
  spawn.rs                  — CLI flag/env propagation, spawn config
  in_process_runner.rs      — in-process teammate execution
  teammate_init.rs          — hook registration, idle notifications
  reconnection.rs           — swarm context init for fresh/resumed sessions
  layout.rs                 — color assignment, pane delegation
  backends/
    mod.rs                  — trait defs, detection, registry
    tmux.rs                 — TmuxBackend (764 LOC in TS)
    iterm2.rs               — ITermBackend (370 LOC in TS)
    in_process.rs           — InProcessBackend (339 LOC in TS)
```
