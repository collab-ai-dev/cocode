# Application Crates — Combined Plan (`app/`)

TS source: `state/`, `bootstrap/`, `components/`, `screens/`, `ink/`, `outputStyles/`, `entrypoints/`, `main.tsx`, `cli/`, `server/`, `bridge/`

All in `app/` directory: query, state, session, tui, cli.
`bridge/` is standalone at workspace root (separate entry point).

## Dependencies (app crates)

```
coco-state depends on:
  - coco-types (ThinkingLevel), coco-config (Settings, FastModeState)
  - coco-tool (Tool trait, ToolPermissionContext — Arc<dyn Tool> in McpState)

coco-session depends on:
  - coco-types (Message, SessionId), coco-messages (SessionCostState)
  - coco-state (AppState)

coco-tui depends on:
  - coco-types, coco-config, coco-state (AppState)
  - coco-query (re-exports `CoreEvent` from coco-types — direct streaming events)
  - coco-commands (CommandRegistry — for command palette)
  - ratatui, crossterm

coco-cli depends on:
  - everything (top-level wiring: creates all registries, injects callbacks, starts runtime)
  - clap (CLI parsing)

coco-bridge depends on:
  - coco-types (Message, Attachment), coco-session (SessionState)
  - tokio-tungstenite (WebSocket transport)

App crates are L5 (top-level wiring). They do NOT have strict
"does NOT depend" constraints — coco-cli depends on everything.
coco-state/coco-session/coco-tui do NOT depend on each other circularly.
```

---

## coco-state

TS source: `state/AppState.tsx`, `state/AppStateStore.ts`, `bootstrap/state.ts`

TS has TWO state tiers:
1. **AppState** — reactive UI state (`Arc<RwLock<AppState>>`, ~80+ fields)
2. **Bootstrap State** — non-reactive process singleton (`bootstrap/state.ts`, ~70 fields)

### AppState (reactive, ~80+ fields)

```rust
/// Global application state (Zustand-like: Arc<RwLock<AppState>>)
pub struct AppState {
    // === Config ===
    pub settings: Settings,
    pub main_loop_model: String,
    pub main_loop_model_for_session: Option<String>,
    pub verbose: bool,

    // === UI Display ===
    pub expanded_view: ExpandedView,      // None, Tasks, Teammates
    pub is_brief_only: bool,
    pub show_teammate_message_preview: bool,
    pub spinner_tip: Option<String>,
    pub footer_selection: Option<FooterItem>,
    pub status_line_text: Option<String>,

    // === Permissions ===
    pub permission_context: ToolPermissionContext,
    pub denial_tracking: DenialTracking,
    pub active_overlays: Vec<OverlayState>,

    // === Tasks & Agents ===
    pub tasks: HashMap<TaskId, TaskState>,
    pub agent_name_registry: HashMap<String, AgentId>, // name → agentId for SendMessage routing
    pub agent_definitions: Vec<AgentDefinition>,
    pub foregrounded_task_id: Option<TaskId>,
    pub viewing_agent_task_id: Option<TaskId>,
    pub selected_ip_agent_index: Option<i32>,
    pub coordinator_task_index: i32,
    pub view_selection_mode: ViewSelectionMode,

    // === MCP ===
    pub mcp: McpState,  // clients, tools, commands, resources, plugin_reconnect_key

    // === Plugins ===
    pub plugins: PluginState, // enabled, disabled, commands, errors, installation_status, needs_refresh

    // === File Tracking ===
    pub file_history: FileHistoryState,
    pub attribution: Option<AttributionState>,
    pub todos: HashMap<AgentId, TodoList>,

    // === Notifications & Elicitation ===
    pub notifications: NotificationState,  // current + queue
    pub elicitation_queue: Vec<ElicitationRequestEvent>,

    // === Hooks & Session ===
    pub session_hooks: SessionHooksState,
    pub auth_version: i64,
    pub initial_message: Option<String>,

    // === Thinking & Suggestions ===
    pub thinking_enabled: Option<bool>,
    pub prompt_suggestion_enabled: bool,
    pub prompt_suggestion: Option<String>,

    // === Mode Flags ===
    pub fast_mode: bool,
    pub advisor_model: Option<String>,
    pub effort_value: Option<ReasoningEffort>,  // from coco-types (low/medium/high/max)
    pub agent: Option<AgentDefinition>,
    pub pending_plan_verification: bool,

    // === Swarm/Teams (v2) ===
    pub team_context: Option<TeamContext>,          // team name, lead/self agent, membership
    pub standalone_agent_context: Option<Value>,
    pub inbox: InboxState,                         // messages: Vec<InboxMessage>
    pub worker_sandbox_permissions: HashMap<String, bool>,
    pub pending_worker_request: Option<Value>,
    pub pending_sandbox_request: Option<Value>,

    // === Bridge/Remote (v3) ===
    // pub repl_bridge: ReplBridgeState,           // 12 fields: enabled, connected, session_active, etc.
    // pub remote_session_url: Option<String>,
    // pub remote_connection_status: ConnectionStatus,
    // pub remote_background_task_count: i32,
    // pub channel_permission_callbacks: HashMap<String, Value>,

    // === Deferred (niche features) ===
    // pub tungsten: Option<TungstenState>,        // tmux panel integration (5 fields)
    // pub bagel: Option<BagelState>,              // web browser tool (3 fields)
    // pub computer_use_mcp_state: Option<Value>,  // macOS computer use (7 sub-fields)
    // pub repl_context: Option<Value>,            // REPL VM sandbox
    // pub speculation: Option<SpeculationState>,  // auto-complete pipelining
    // pub ultraplan: Option<UltraplanState>,      // remote plan generation (5 fields)
    // pub companion: Option<CompanionState>,      // buddy pet (2 fields)
}

### Bootstrap State (non-reactive process singleton, ~70 fields)

TS `bootstrap/state.ts` holds process-lifetime state that does NOT need UI reactivity.
In Rust this maps to a `static` or a `OnceCell<Arc<BootstrapState>>`.

Key field categories (not exhaustive — see TS source for full list):
- **Session identity**: session_id, parent_session_id, project_root, original_cwd, cwd
- **Cost/timing accumulators**: total_cost_usd, total_api_duration, total_tool_duration, total_lines_added/removed
- **Model overrides**: main_loop_model_override, initial_main_loop_model, model_strings
- **Beta header latches** (session-stable, prevent cache busting): afk_mode_header_latched, fast_mode_header_latched, cache_editing_header_latched, thinking_clear_latched
- **Prompt cache state**: prompt_cache_1h_allowlist, prompt_cache_1h_eligible
- **Session flags**: is_interactive, session_bypass_permissions_mode, session_persistence_disabled, has_exited_plan_mode, needs_plan_mode_exit_attachment
- **Skills & cron**: invoked_skills (Set), session_cron_tasks, plan_slug_cache (Map<SessionId, String>)
- **Telemetry handles**: meter, session_counter, cost_counter, token_counter, logger_provider, tracer_provider
- **API debug**: last_api_request, last_classifier_requests, cached_claude_md_content
- **Client type**: client_type (12 variants: cli, sdk-cli, sdk-typescript, sdk-python, remote, claude-vscode, etc.)

### onChangeAppState Side-Effects

A single handler that fires on every AppState mutation:
- Syncs `permission_context.mode` changes to CCR via `notifySessionMetadataChanged`
- Persists `main_loop_model` changes to user settings
- Persists `expanded_view` and `verbose` to global config
- Clears auth caches on `settings` change
- Re-applies env vars on `settings.env` change

pub struct NotificationState {
    pub current: Option<Notification>,
    pub queue: Vec<Notification>,
}

pub struct McpState {
    pub clients: Vec<McpConnection>,
    pub tools: Vec<Arc<dyn Tool>>,
    pub commands: Vec<Command>,
    pub resources: HashMap<String, Vec<ServerResource>>,
}

pub struct PluginState {
    pub enabled: Vec<LoadedPlugin>,
    pub disabled: Vec<LoadedPlugin>,
    pub commands: Vec<Command>,
    pub errors: Vec<PluginError>,
}
```

### Core Logic

```rust
pub struct AppStateStore {
    state: Arc<RwLock<AppState>>,
    subscribers: Vec<Box<dyn Fn() + Send + Sync>>,
}

impl AppStateStore {
    pub fn new() -> Self;
    pub fn get(&self) -> AppState;  // clone current state
    pub fn set(&self, updater: impl FnOnce(&mut AppState));
    pub fn subscribe(&mut self, listener: impl Fn() + Send + Sync + 'static) -> usize;
}

pub fn get_default_app_state() -> AppState;
```

---

## coco-session

TS source: `bootstrap/state.ts`, session management

### Data Definitions

```rust
pub struct Session {
    pub id: SessionId,
    pub created_at: i64,
    pub updated_at: i64,
    pub cwd: PathBuf,
    pub model: String,
    pub title: Option<String>,
}

pub struct SessionState {
    pub session: Session,
    pub messages: Vec<Message>,
    pub cost_state: SessionCostState,
    pub file_state_cache: FileStateCache,
}
```

### Core Logic

```rust
pub struct SessionManager {
    sessions_dir: PathBuf,  // ~/.coco/sessions/
}

impl SessionManager {
    pub fn create(&self, cwd: &Path, model: &str) -> Session;
    pub fn load(&self, id: &SessionId) -> Result<SessionState, SessionError>;
    pub fn save(&self, state: &SessionState);
    pub fn list(&self) -> Vec<Session>;
    pub fn resume(&self, id: &SessionId) -> Result<SessionState, SessionError>;
    pub fn delete(&self, id: &SessionId);
}
```

---

## coco-tui

TS source: `components/`, `screens/`, `ink/`, `outputStyles/`

### Architecture: TEA (The Elm Architecture) with ratatui

```rust
pub struct App {
    state: AppState,
    ui_state: UiState,
}

pub struct UiState {
    pub input_text: String,
    pub cursor_pos: usize,
    pub scroll_offset: usize,
    pub mode: UiMode,  // Normal, CommandPalette, PermissionDialog, SessionBrowser
    pub notification: Option<Notification>,
}

pub enum UiEvent {
    Key(KeyEvent),
    CoreEvent(coco_types::CoreEvent),  // 3-layer: Protocol/Stream/Tui
    Resize(u16, u16),
    Tick,
}

impl App {
    pub async fn run(&mut self, terminal: &mut Terminal<impl Backend>) -> Result<(), AppError> {
        loop {
            terminal.draw(|f| self.render(f))?;
            match self.handle_event(next_event().await) {
                Action::Continue => {}
                Action::Quit => break,
                Action::Submit(input) => self.submit(input).await?,
            }
        }
    }
}
```

### Key Widgets

- `ChatWidget` — message list with markdown rendering
- `InputWidget` — user input with autocomplete
- `SpinnerWidget` — progress indicator with streaming deltas
- inline tool status rows — tool execution progress inside the transcript
- `PermissionDialogWidget` — permission approval overlay
- `CommandPaletteWidget` — fuzzy command search
- `SessionBrowserWidget` — session list with search
- `StatusBarWidget` — model, cost, context usage

---

## coco-cli

TS source: `main.tsx` (~4500 LOC), `entrypoints/cli.tsx`, `cli/`

### Two-Tier Dispatch Architecture

TS has a two-tier entry point: `cli.tsx` handles fast-path dispatch (zero module loading)
before `main.tsx` constructs the full Commander program.

**Fast-path branches** (in `cli.tsx`, before Commander.js):
- `--version / -v / -V` — zero module loading
- `--dump-system-prompt` — ant-internal eval tool
- `--chrome-native-host`, `--computer-use-mcp` — in-process MCP servers
- `--daemon-worker` — feature-gated worker process
- `remote-control | rc | remote | sync | bridge` — CCR bridge mode
- `daemon`, `ps | logs | attach | kill | --bg` — background session management
- `--handle-uri` — deep link / URL scheme handler

### Subcommand Tree (~50+)

```
coco                          — interactive TUI (default)
coco -p "prompt"              — print mode (non-interactive)
coco --sdk-url <url>          — SDK mode (structured IO via remote endpoint)

coco mcp serve|add|remove|list|get|add-json|add-from-claude-desktop|reset-project-choices
coco auth login|logout|status
coco plugin list|validate|install|uninstall|enable|disable|update
coco plugin marketplace add|list|remove|update
coco doctor
coco update (alias: upgrade)
coco agents
coco setup-token
coco completion <shell>
coco server --port --host --unix --auth-token --workspace --idle-timeout --max-sessions
coco ssh <host> [dir]
coco open <cc-url>
coco remote-control
coco assistant [sessionId]
coco auto-mode defaults|config|critique

Feature-gated:
  coco daemon [subcommand]
  coco ps|logs|attach|kill       (background sessions)
  coco new|list|reply            (templates)
  coco environment-runner        (BYOC runner)
  coco task create|list|get|update|dir
```

### Key Flag Categories (60+)

| Category | Flags |
|----------|-------|
| Session | `-p/--print`, `--output-format`, `--input-format`, `--json-schema`, `--max-turns`, `--max-budget-usd` |
| Resume | `-c/--continue`, `-r/--resume`, `--fork-session`, `--from-pr`, `--session-id`, `-n/--name`, `--rewind-files` |
| Auth/Perms | `--dangerously-skip-permissions`, `--permission-mode`, `--permission-prompt-tool` |
| Tools | `--allowed-tools`, `--disallowed-tools`, `--tools` |
| Config | `--settings`, `--setting-sources`, `--system-prompt`, `--mcp-config`, `--add-dir`, `--plugin-dir` |
| Model | `--model`, `--betas`, `--agent`, `--agents`, `--thinking`, `--effort`, `--fallback-model` |
| Worktree | `-w/--worktree`, `--tmux` |
| Misc | `--bare`, `--init`, `--verbose`, `--file`, `--ide`, `--prefill`, `--disable-slash-commands` |

### Startup Flow (expanded)

```
1. Early argv parsing: MDM raw read + keychain prefetch (parallelized)
2. Fast-path dispatch (15 branches, zero module load for --version)
3. Client type determination (12 variants: cli, sdk-cli, sdk-typescript, sdk-python, remote, etc.)
4. Eager settings load (--settings, --setting-sources before init())
5. Commander program construction with preAction hook
6. preAction: ensureMdmSettingsLoaded → init() → runMigrations (11 versioned) → loadRemoteManagedSettings → loadPolicyLimits
7. Telemetry init (deferred until after trust dialog)
8. Enter mode (TUI, print, SDK)
9. Deferred prefetches (after first render): credentials, file count, MCP, model capabilities
```

### StructuredIO SDK Protocol (21 control subtypes)

cocode-rs reference: `app/cli/src/sdk.rs` (SDK mode entry), `app/cli/src/transport.rs` (NDJSON transport),
`common/protocol/src/lib.rs` (ServerNotification 43 variants, ClientRequest 22, ServerRequest 5).
The event system design from cocode-rs is reused — see `docs/coco-rs/event-system-design.md` for the
3-layer architecture (Protocol → Stream → TUI) and StreamAccumulator state machine.

**Phase 0 status** (April 2026): `CoreEvent`, `ServerNotification` (52 variants = 43 base +
9 TS gaps + `HookExecuted` kept from cocode-rs base = 53 actual), `AgentStreamEvent` (7 variants),
`TuiOnlyEvent` (20 variants), `ThreadItem`/`ItemStatus`, and `StreamAccumulator` are all
implemented in `coco-types` and `coco-query`. `ClientRequest`/`ServerRequest` control protocol
and SDK stdio transport are Phase 2 work.

Bidirectional NDJSON protocol over stdin/stdout for programmatic SDK use:

**Control request subtypes** (SDK host → agent):
```
initialize, interrupt, can_use_tool, set_permission_mode, set_model,
set_max_thinking_tokens, mcp_status, get_context_usage, hook_callback,
mcp_message, rewind_files, cancel_async_message, seed_read_state,
mcp_set_servers, reload_plugins, mcp_reconnect, mcp_toggle, stop_task,
apply_flag_settings, get_settings, elicitation
```
Plus: `keep_alive`, `update_environment_variables`, `control_cancel_request`

**SDK output messages** (agent → SDK host, 22+ types):
```
assistant, user, stream_event, result
system/init, system/status, system/compact_boundary, system/post_turn_summary
system/api_retry, system/hook_started, system/hook_progress, system/hook_response
system/files_persisted, system/task_notification, system/task_started, system/task_progress
system/session_state_changed (idle/running/requires_action)
system/elicitation_complete, system/local_command_output
tool_progress, tool_use_summary, rate_limit_event
```

**Permission racing**: hook evaluation races against SDK permission prompt (whichever resolves first wins).
**Duplicate dedup**: LRU set of 1000 resolved tool_use IDs prevents double-processing.

### Transport Types (4 concrete implementations)

| Transport | Description |
|-----------|-------------|
| `WebSocketTransport` | Default: WS read + WS write |
| `HybridTransport` | V2: WS read + HTTP POST write (batched, with retry/backpressure) |
| `SSETransport` | CCR v2: SSE read + HTTP POST write |
| `CCRClient` | Used from RemoteIO for CCR-specific session management |

Selection priority:
1. SSETransport when `CLAUDE_CODE_USE_CCR_V2` set
2. HybridTransport when `CLAUDE_CODE_POST_FOR_SESSION_INGRESS_V2` set
3. WebSocketTransport (default fallback)

Retry/timeout constants:
- POST_TIMEOUT_MS: 15,000 (HybridTransport per-attempt)
- LIVENESS_TIMEOUT_MS: 45,000 (SSETransport liveness check)
- CCRClient default timeout: 10,000 (requests), 30,000 (GET), 5,000 (token refresh)
- Exponential backoff with jitter; respects Retry-After headers from 429/5xx

---

## coco-bridge

TS source: `bridge/` (CCR daemon), `server/` (DirectConnect)

### Architecture

**NOTE**: The bridge is NOT a direct IDE↔agent WebSocket relay.
The TS implementation has TWO distinct subsystems:

1. **CCR Bridge** (`bridge/`): A standalone daemon process that registers as an
   "environment" with the Anthropic backend (REST), long-polls for work items, spawns
   child `claude` processes per session, and relays events. It has 3 spawn modes
   (`single-session`, `worktree`, `same-dir`), JWT-based session ingress auth,
   V1 (WebSocket) and V2 (HybridTransport: WS read + HTTP POST write) transports.

2. **DirectConnect Server** (`server/`): A local HTTP+WebSocket server
   (`POST /sessions` → `{session_id, ws_url, work_dir}`) for programmatic SDK use.
   Session lifecycle: starting → running → detached → stopping → stopped.
   NDJSON codec: WebSocket uses newline-delimited JSON, each line parsed independently.
   Session index: persisted to `~/.coco/server-sessions.json` for cross-restart resume.
   Config: port, host, authToken, idleTimeoutMs (0=never), maxSessions, workspace.
   ```rust
   pub enum DirectConnectSessionState { Starting, Running, Detached, Stopping, Stopped }

   pub struct ServerConfig {
       pub port: i32,
       pub host: String,
       pub auth_token: String,
       pub unix_socket: Option<String>,
       pub idle_timeout_ms: Option<i64>,  // 0 = never expire
       pub max_sessions: Option<i32>,
       pub workspace: Option<String>,
   }

   /// Persisted to ~/.coco/server-sessions.json for cross-restart resume.
   pub struct SessionIndexEntry {
       pub session_id: String,
       pub transcript_session_id: String,  // for --resume
       pub cwd: String,
       pub permission_mode: Option<String>,
       pub created_at: i64,
       pub last_active_at: i64,
   }
   ```

3. **IDE Integration**: IDEs connect as **MCP servers** (not via the bridge).
   `useIDEIntegration` detects IDE via lockfiles at `~/.claude/ide/<port>.lock`,
   registers as `sse-ide` or `ws-ide` MCP transport type. 17 IDE types supported
   (VS Code, Cursor, Windsurf + 14 JetBrains IDEs). IDE sends 4 MCP notification
   types: `selection_changed`, `at_mentioned`, `log_event`, `ide_connected`.
   Outbound RPC: `openDiff`, `close_tab`, `openFile`, `getDiagnostics`.

```rust
// Full bridge spec: see crate-coco-bridge.md (dedicated doc created for P0 gap).
// Below are summary types — canonical definitions in crate-coco-bridge.md.

pub struct BridgeConfig {
    pub bridge_id: String,
    pub environment_id: Option<String>,
    pub worker_type: BridgeWorkerType, // claude_code | claude_code_assistant
    pub spawn_mode: SpawnMode,         // SingleSession | Worktree | SameDir
    pub max_sessions: i32,
    pub session_ingress_url: Option<String>,
    pub session_timeout_ms: Option<i64>,
}

pub enum SpawnMode { SingleSession, Worktree, SameDir }
pub enum BridgeWorkerType { ClaudeCode, ClaudeCodeAssistant }
```
