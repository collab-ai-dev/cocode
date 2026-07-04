//! Task lifecycle types — the unified seam across all running tasks
//! (backgrounded agents, shells, in-process teammates, future remote
//! teammates, dream consolidation).
//!
//! ## Variant renames
//!
//! The refactor renamed TS's confusing names for Rust clarity. These
//! are wire-breaking; old transcripts won't deserialize cleanly.
//!
//! | Concept | TS | Rust |
//! |----------------------|-----------------------|---------------|
//! | Backgrounded agent | `local_agent` | `bg_agent` |
//! | Bash background | `local_bash` | `shell` |
//! | Local teammate | `in_process_teammate` | `teammate` |
//! | Remote teammate | `remote_agent` | `remote_teammate` |
//! | Dream | `dream` | `dream` |
//!
//! "local agent" in TS actually meant "backgrounded subagent task". The
//! primary REPL is not a task and was never in this taxonomy. Rust
//! says what it means.
//!
//! ## Why the variant slot for [`RemoteTeammate`](TaskExtras::RemoteTeammate)
//! exists with no driver yet
//!
//! Forcing every consumer to consider the remote variant in `match` arms
//! is the whole point — it stops the "unify the abstraction by deleting
//! the variant that breaks it" pattern. When Teleport / CCR support
//! lands the body of [`RemoteTeammateExtras`] gets fleshed out; today
//! the variant compiles but is never constructed.

use serde::Deserialize;
use serde::Serialize;

// ─── Backends ──────────────────────────────────────────────────────────

/// Backend that drives a teammate's execution.
/// `#[non_exhaustive]` — future backends (Wezterm, Kitty, Windows
/// Terminal panes) can land without a major version bump.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum BackendType {
    Tmux,
    Iterm2,
    InProcess,
}

impl BackendType {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Tmux => "tmux",
            Self::Iterm2 => "iterm2",
            Self::InProcess => "in-process",
        }
    }

    pub const fn is_pane_backend(&self) -> bool {
        matches!(self, Self::Tmux | Self::Iterm2)
    }
}

// ─── TeammateRef ───────────────────────────────────────────────────────

/// Identity of an agent participating in a team — `name@team`.
/// Parsed into a struct so consumers can read the two halves without
/// re-splitting at every call site. Wire format is the single
/// `"name@team"` string for round-trip with TS transcripts.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TeammateRef {
    pub name: String,
    pub team: String,
}

impl TeammateRef {
    pub fn new(name: impl Into<String>, team: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            team: team.into(),
        }
    }

    /// Parse the wire form `"name@team"`. Empty name or team is rejected.
    pub fn parse(s: &str) -> Option<Self> {
        let (name, team) = s.split_once('@')?;
        if name.is_empty() || team.is_empty() {
            return None;
        }
        Some(Self {
            name: name.to_string(),
            team: team.to_string(),
        })
    }
}

impl std::fmt::Display for TeammateRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.name, self.team)
    }
}

impl Serialize for TeammateRef {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for TeammateRef {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).ok_or_else(|| {
            serde::de::Error::custom(format!("invalid TeammateRef '{s}' (expected 'name@team')"))
        })
    }
}

// ─── TaskIdentity ──────────────────────────────────────────────────────

/// Per-variant identity, computed from a task row's extras. Used as a
/// uniform dispatch shape — consumers that route on what kind of thing
/// a task represents match on [`TaskIdentity`] rather than re-implementing
/// the dispatch.
/// Reserves the [`RemoteTeammate`](Self::RemoteTeammate) slot so future
/// remote-agent support cannot be added by deleting variants out of the
/// abstraction — the slot exists today; populating it later won't
/// require rewriting consumer call sites.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskIdentity<'a> {
    /// Backgrounded subagent task. Identity is the row id itself.
    BgAgent(&'a str),
    /// Local in-process teammate. Identity is `name@team`.
    Teammate(&'a TeammateRef),
    /// Remote-controlled teammate (Teleport / CCR). Identity is a remote
    /// session id. The slot exists but is not yet constructed in code.
    RemoteTeammate { session_id: &'a str },
    /// Background shell task. Optional issuing-agent context.
    Shell { issuing_agent: Option<&'a str> },
    /// Local workflow task. Identity is the row id itself.
    LocalWorkflow(&'a str),
    /// Dream task (memory consolidation).
    Dream,
}

// ─── Progress ──────────────────────────────────────────────────────────

/// Progress snapshot for a running task. TS: `agentToolUtils.ts`
/// Progress tracking for local agent tasks.
/// Lives on the per-variant extras (BgAgent / Teammate / RemoteTeammate)
/// because shell + dream tasks have no progress concept; sparse Options
/// at the base level were the previous design and were hiding the
/// semantic asymmetry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskProgress {
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(default)]
    pub total_tokens: i64,
    /// Input tokens served from the prompt cache (the `cache hit`
    /// dimension). Mirrors `TokenUsage.input_tokens.cache_read`.
    #[serde(default)]
    pub cache_read_tokens: i64,
    /// Real cost in micro-USD (USD × 1_000_000). Subagent progress
    /// snapshots can populate this mid-flight from session usage updates;
    /// terminal transitions stamp the final value before `TaskCompleted`.
    /// Integer to keep `TaskProgress: Eq` (and the `TaskStateBase` chain
    /// that embeds it).
    #[serde(default)]
    pub cost_micro_usd: i64,
    /// Input-side / output-side cost split (micro-USD, to keep
    /// `TaskProgress: Eq`), stamped alongside `cost_micro_usd` so the TUI
    /// can show subagent spend as `↑…/$in ↓…/$out` like the main thread.
    #[serde(default)]
    pub input_cost_micro_usd: i64,
    #[serde(default)]
    pub output_cost_micro_usd: i64,
    /// Declared subagent type (`Explore` / `Plan` / …). Carried on
    /// progress so the TUI can replace the `local_agent` wire fallback
    /// (which is all `TaskStarted` knows) with the real type once the
    /// first progress arrives. `None` for non-subagent tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    #[serde(default)]
    pub tool_use_count: i32,
    #[serde(default)]
    pub turn_count: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tool_name: Option<String>,
    /// Most-recent tool activities for this task, capped at the
    /// caller's ring-buffer size (typically 3 — see TS
    /// Newest entries
    /// are pushed onto the end; clients render in display order
    /// (oldest of the last N first). Empty when the task hasn't
    /// invoked tools yet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_activities: Vec<TaskActivity>,
    /// 1-2 sentence summary from the periodic AgentSummary timer.
    /// Independent of token deltas — preserved across overlapping
    /// progress writes (the only writer is the summary timer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// Per-tool-invocation activity row for the in-process teammate
/// spinner tree.
/// Carries the tool name and an optional one-line summary; the
/// caller decides how to render. Designed to ride in
/// [`TaskProgress::recent_activities`] as a small ring buffer.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskActivity {
    /// Canonical tool name (e.g. `Read`, `Bash`, `Edit`).
    pub tool_name: String,
    /// Short, single-line activity descriptor — typically the first
    /// arg or a file path. `None` when no useful summary is
    /// available; the renderer should fall back to `tool_name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

// ─── Teammate UI message ───────────────────────────────────────────────

/// Role of a message in the teammate UI mirror.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
    Tool,
}

/// Capped conversation preview for an in-process teammate task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeammateTaskMessage {
    pub role: MessageRole,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

// ─── TaskType / TaskStatus ─────────────────────────────────────────────

/// Top-level discriminator. Closed enum; every consumer that pattern-
/// matches on it must handle every variant — including
/// [`Self::RemoteTeammate`] which has no driver yet (today its match
/// arm typically calls `unimplemented!()` or returns "unsupported in
/// this build").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    BgAgent,
    Shell,
    LocalWorkflow,
    Teammate,
    RemoteTeammate,
    Dream,
}

impl TaskType {
    /// wire string for the `task_type` field on the
    /// `task/started`, `task/progress`, and `task/completed` events.
    /// See [`task_type_wire`] for the same
    /// constants exposed as module items, useful when matching against
    /// `Option<String>` from a deserialized payload.
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::Shell => task_type_wire::LOCAL_BASH,
            Self::BgAgent => task_type_wire::LOCAL_AGENT,
            Self::LocalWorkflow => task_type_wire::LOCAL_WORKFLOW,
            Self::Teammate => task_type_wire::IN_PROCESS_TEAMMATE,
            Self::RemoteTeammate => task_type_wire::REMOTE_AGENT,
            Self::Dream => task_type_wire::DREAM,
        }
    }
}

/// wire-string constants for [`TaskType`]. Use these on
/// both producer and consumer sides instead of raw string literals —
/// drift between [`TaskType::wire_name`] and a match arm becomes a
/// compile error rather than a silent miss.
pub mod task_type_wire {
    pub const LOCAL_BASH: &str = "local_bash";
    pub const LOCAL_AGENT: &str = "local_agent";
    pub const LOCAL_WORKFLOW: &str = "local_workflow";
    pub const IN_PROCESS_TEAMMATE: &str = "in_process_teammate";
    pub const REMOTE_AGENT: &str = "remote_agent";
    pub const DREAM: &str = "dream";
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Killed,
}

impl TaskStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Killed)
    }
}

/// Actor that caused a running task to be stopped.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskKilledBy {
    User,
    Parent,
    System,
}

impl TaskKilledBy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Parent => "parent",
            Self::System => "system",
        }
    }
}

// ─── Per-variant extras ────────────────────────────────────────────────

/// Backgrounded subagent task sidecar.
/// Holds variant-owned `progress` and `is_backgrounded` (previously
/// hoisted to the base, but sparse for shell / teammate / dream).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BgAgentExtras {
    /// Live progress snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<TaskProgress>,
    /// Foreground (false) vs detached (true).
    #[serde(default)]
    pub is_backgrounded: bool,
    /// True once `TaskOutputTool` reads the terminal output. Stops the
    /// compact reminder re-announcing the same agent. TS:
    /// `agent.retrieved` tag.
    #[serde(default)]
    pub retrieved: bool,
    /// UI pin — blocks panel eviction.
    #[serde(default)]
    pub retain: bool,
    /// Unix-ms deadline after which the panel may evict the task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evict_after: Option<i64>,
    /// Error text from a `Failed` terminal transition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Shell task sidecar.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellExtras {
    /// UI display variant — `bash` (default) or `monitor`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The command string.
    #[serde(default)]
    pub command: String,
    /// Agent that issued the spawn (completion notifications get routed
    /// to this agent's filter). Stringly-typed at the wire level; the
    /// canonical format is the BgAgent id (`a<16hex>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuing_agent: Option<String>,
    /// Exit code once known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Foreground (false) vs detached (true). Owned by the variant
    /// because teammate / dream tasks don't have this semantic.
    #[serde(default)]
    pub is_backgrounded: bool,
}

/// Dream task sidecar — empty. Kept as a distinct variant so future
/// dream-specific fields (consolidation stats, target memory dir, …)
/// don't require a wire-format migration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DreamExtras {}

/// Local workflow task sidecar.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalWorkflowExtras {
    /// Run id (`wf_…`) of this workflow invocation. Recorded so a later
    /// `resumeFromRunId` can map an arbitrary prior run back to its task row
    /// (and thus its persisted script + journal). Same-session only.
    #[serde(default)]
    pub run_id: String,
    /// Workflow display name, when known from script metadata or input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_name: Option<String>,
    /// Prompt passed into the workflow, when supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Streaming workflow state deltas.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workflow_progress: Vec<WorkflowProgressEvent>,
    /// Foreground (false) vs detached (true).
    #[serde(default)]
    pub is_backgrounded: bool,
}

/// Typed workflow progress payload carried by `task/progress`.
/// Mirrors Claude's workflow progress wire shape. `WorkflowAgent` is much
/// larger than phase/log events, but boxing the variant fields would add
/// indirection to every producer and consumer of this serde payload for no
/// practical win: values are stored in task progress vectors and rendered or
/// forwarded, not packed into hot inner-loop collections.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum WorkflowProgressEvent {
    WorkflowAgent {
        index: i32,
        state: WorkflowAgentState,
        label: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        phase_title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        phase_index: Option<i32>,
        /// Spawned subagent id, once running.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
        /// Resolved model id for the subagent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        /// Epoch millis when the agent started running.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        started_at: Option<i64>,
        /// Epoch millis when the agent entered the workflow queue.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        queued_at: Option<i64>,
        /// Epoch millis for the latest observed progress heartbeat.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last_progress_at: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tokens: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_calls: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<i64>,
        /// Cache-replay hit: emitted as `state: done` with `cached: true`
        /// (TS does not use a distinct state for cache hits).
        #[serde(default)]
        cached: bool,
        /// Truncated preview of the subagent result.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result_preview: Option<String>,
        /// Truncated preview of the subagent prompt.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt_preview: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        /// Error events produced by a user skip are displayed separately
        /// from ordinary failures.
        #[serde(default)]
        skipped: bool,
    },
    WorkflowPhase {
        index: i32,
        title: String,
    },
    WorkflowLog {
        message: String,
    },
}

/// Per-agent lifecycle state (`start` → `progress` → `done`, or `error`).
/// A cache-replay hit is reported
/// as `Done` with the sibling `cached: true` flag, not a distinct state.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowAgentState {
    Start,
    Progress,
    Done,
    Error,
}

/// Local in-process teammate sidecar.
/// `agent_ref` is the parsed `name@team` identity — replaces the old
/// duplicate `agent_id` / `agent_name` / `team_name` fields. Progress
/// lives here (not on the base) because it's variant-owned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeammateExtras {
    /// `name@team` identity. Wire format: a single string.
    pub agent_ref: TeammateRef,
    /// Live progress snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<TaskProgress>,
    /// Which backend drives this teammate. Required — a teammate
    /// without a backend cannot be spawned.
    pub backend_type: BackendType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
    pub prompt: String,
    #[serde(default)]
    pub is_idle: bool,
    #[serde(default)]
    pub shutdown_requested: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<TeammateTaskMessage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_user_messages: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spinner_verb: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub past_tense_verb: Option<String>,
}

impl TeammateExtras {
    pub fn new(agent_ref: TeammateRef, backend_type: BackendType, prompt: String) -> Self {
        Self {
            agent_ref,
            progress: None,
            backend_type,
            pane_id: None,
            prompt,
            is_idle: false,
            shutdown_requested: false,
            error: None,
            result: None,
            messages: Vec::new(),
            pending_user_messages: Vec::new(),
            spinner_verb: None,
            past_tense_verb: None,
        }
    }
}

/// Remote-controlled teammate sidecar. **Reserved variant slot** — no
/// driver in coco-rs today. The fields below are placeholders informed
/// by the `RemoteAgentTaskState` shape (`sessionId`,
/// `remoteTaskType`, `log: SDKMessage[]`); the variant exists so
/// every consumer match arm must consider remote teammates rather
/// than the abstraction being "unified" by deleting the inconvenient
/// variant.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteTeammateExtras {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<TaskProgress>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

/// Per-`TaskType` sidecar extras. Rust analog of TS's
/// `LocalShellTaskState | LocalAgentTaskState | InProcessTeammateTaskState | …`
/// union.
/// Wire shape: flattened onto [`TaskStateBase`] via `#[serde(flatten)]`,
/// dispatched on the parent's `type` discriminator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaskExtras {
    BgAgent(BgAgentExtras),
    Shell(ShellExtras),
    LocalWorkflow(LocalWorkflowExtras),
    Teammate(TeammateExtras),
    RemoteTeammate(RemoteTeammateExtras),
    Dream(DreamExtras),
}

impl TaskExtras {
    pub fn task_type(&self) -> TaskType {
        match self {
            Self::BgAgent(_) => TaskType::BgAgent,
            Self::Shell(_) => TaskType::Shell,
            Self::LocalWorkflow(_) => TaskType::LocalWorkflow,
            Self::Teammate(_) => TaskType::Teammate,
            Self::RemoteTeammate(_) => TaskType::RemoteTeammate,
            Self::Dream(_) => TaskType::Dream,
        }
    }

    pub fn bg_agent(&self) -> Option<&BgAgentExtras> {
        if let Self::BgAgent(e) = self {
            Some(e)
        } else {
            None
        }
    }

    pub fn bg_agent_mut(&mut self) -> Option<&mut BgAgentExtras> {
        if let Self::BgAgent(e) = self {
            Some(e)
        } else {
            None
        }
    }

    pub fn shell(&self) -> Option<&ShellExtras> {
        if let Self::Shell(e) = self {
            Some(e)
        } else {
            None
        }
    }

    pub fn shell_mut(&mut self) -> Option<&mut ShellExtras> {
        if let Self::Shell(e) = self {
            Some(e)
        } else {
            None
        }
    }

    pub fn teammate(&self) -> Option<&TeammateExtras> {
        if let Self::Teammate(e) = self {
            Some(e)
        } else {
            None
        }
    }

    pub fn teammate_mut(&mut self) -> Option<&mut TeammateExtras> {
        if let Self::Teammate(e) = self {
            Some(e)
        } else {
            None
        }
    }

    pub fn remote_teammate(&self) -> Option<&RemoteTeammateExtras> {
        if let Self::RemoteTeammate(e) = self {
            Some(e)
        } else {
            None
        }
    }

    pub fn remote_teammate_mut(&mut self) -> Option<&mut RemoteTeammateExtras> {
        if let Self::RemoteTeammate(e) = self {
            Some(e)
        } else {
            None
        }
    }

    /// Progress snapshot if this variant has one.
    pub fn progress(&self) -> Option<&TaskProgress> {
        match self {
            Self::BgAgent(e) => e.progress.as_ref(),
            Self::Teammate(e) => e.progress.as_ref(),
            Self::RemoteTeammate(e) => e.progress.as_ref(),
            Self::Shell(_) | Self::LocalWorkflow(_) | Self::Dream(_) => None,
        }
    }

    /// Mutable progress slot — used by the runtime's `set_progress`
    /// path. Returns `None` for variants without a progress slot
    /// (shell, dream) so writes to them are explicit no-ops.
    pub fn progress_slot_mut(&mut self) -> Option<&mut Option<TaskProgress>> {
        match self {
            Self::BgAgent(e) => Some(&mut e.progress),
            Self::Teammate(e) => Some(&mut e.progress),
            Self::RemoteTeammate(e) => Some(&mut e.progress),
            Self::Shell(_) | Self::LocalWorkflow(_) | Self::Dream(_) => None,
        }
    }

    /// Backgrounded flag (variant-owned). Teammate and remote variants
    /// always report `false`.
    pub fn is_backgrounded(&self) -> bool {
        match self {
            Self::BgAgent(e) => e.is_backgrounded,
            Self::Shell(e) => e.is_backgrounded,
            Self::LocalWorkflow(e) => e.is_backgrounded,
            Self::Teammate(_) | Self::RemoteTeammate(_) | Self::Dream(_) => false,
        }
    }

    /// Set backgrounded if this variant tracks it. Returns true when
    /// the variant accepted the write.
    pub fn set_backgrounded(&mut self, value: bool) -> bool {
        match self {
            Self::BgAgent(e) => {
                e.is_backgrounded = value;
                true
            }
            Self::Shell(e) => {
                e.is_backgrounded = value;
                true
            }
            Self::LocalWorkflow(e) => {
                e.is_backgrounded = value;
                true
            }
            _ => false,
        }
    }

    pub fn bg_agent_default() -> Self {
        Self::BgAgent(BgAgentExtras::default())
    }

    pub fn shell_default() -> Self {
        Self::Shell(ShellExtras::default())
    }

    pub fn dream() -> Self {
        Self::Dream(DreamExtras::default())
    }

    pub fn local_workflow(
        run_id: String,
        workflow_name: Option<String>,
        prompt: Option<String>,
    ) -> Self {
        Self::LocalWorkflow(LocalWorkflowExtras {
            run_id,
            workflow_name,
            prompt,
            ..LocalWorkflowExtras::default()
        })
    }

    pub fn local_workflow_extras(&self) -> Option<&LocalWorkflowExtras> {
        if let Self::LocalWorkflow(e) = self {
            Some(e)
        } else {
            None
        }
    }
}

// ─── TaskStateBase ─────────────────────────────────────────────────────

/// Canonical task row — one per running thing.
/// Cancellation tokens and watch channels do NOT live here; they live
/// in a sibling `TaskControl` map on the runtime so this struct stays
/// pure serializable wire data.
/// Per-task-type fields (progress, is_backgrounded, retain, command,
/// agent_ref, …) live in [`TaskExtras`], flattened onto the wire so
/// the JSON shape matches a discriminated union.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskStateBase {
    /// Stringly-typed for caller ergonomics — the typed [`TaskId`] /
    /// [`AgentId`] newtypes are available in `coco_types::id` for
    /// callers that want them and are adopted incrementally. The
    /// canonical format matches the variant prefix scheme used by
    /// [`generate_task_id`].
    pub id: String,
    pub status: TaskStatus,
    /// Latch — terminal notifications and panel eviction are idempotent.
    #[serde(default)]
    pub notified: bool,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    pub start_time: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_time: Option<i64>,
    /// Actor that stopped this task. Present only for killed terminal
    /// transitions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub killed_by: Option<TaskKilledBy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_paused_ms: Option<i64>,
    /// Path to the on-disk file backing this task's captured output.
    /// `None` for tasks that produce no output (e.g. Dream); the empty
    /// string overload was ambiguous (no-file vs file-not-yet-written).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_file: Option<String>,
    #[serde(default)]
    pub output_offset: i64,
    /// Variant-specific fields, flattened onto the wire.
    #[serde(flatten)]
    pub extras: TaskExtras,
}

impl TaskStateBase {
    pub fn task_type(&self) -> TaskType {
        self.extras.task_type()
    }

    /// Compute the per-variant identity. Borrowing accessor so callers
    /// can match without cloning.
    pub fn identity(&self) -> TaskIdentity<'_> {
        match &self.extras {
            TaskExtras::BgAgent(_) => TaskIdentity::BgAgent(self.id.as_str()),
            TaskExtras::LocalWorkflow(_) => TaskIdentity::LocalWorkflow(self.id.as_str()),
            TaskExtras::Teammate(e) => TaskIdentity::Teammate(&e.agent_ref),
            TaskExtras::RemoteTeammate(e) => TaskIdentity::RemoteTeammate {
                session_id: e.session_id.as_str(),
            },
            TaskExtras::Shell(e) => TaskIdentity::Shell {
                issuing_agent: e.issuing_agent.as_deref(),
            },
            TaskExtras::Dream(_) => TaskIdentity::Dream,
        }
    }

    pub fn progress(&self) -> Option<&TaskProgress> {
        self.extras.progress()
    }

    pub fn is_backgrounded(&self) -> bool {
        self.extras.is_backgrounded()
    }

    pub fn bg_agent_extras(&self) -> Option<&BgAgentExtras> {
        self.extras.bg_agent()
    }

    pub fn bg_agent_extras_mut(&mut self) -> Option<&mut BgAgentExtras> {
        self.extras.bg_agent_mut()
    }

    pub fn shell_extras(&self) -> Option<&ShellExtras> {
        self.extras.shell()
    }

    pub fn shell_extras_mut(&mut self) -> Option<&mut ShellExtras> {
        self.extras.shell_mut()
    }

    pub fn teammate_extras(&self) -> Option<&TeammateExtras> {
        self.extras.teammate()
    }

    pub fn teammate_extras_mut(&mut self) -> Option<&mut TeammateExtras> {
        self.extras.teammate_mut()
    }

    pub fn retain(&self) -> bool {
        self.bg_agent_extras().map(|e| e.retain).unwrap_or(false)
    }

    pub fn retrieved(&self) -> bool {
        self.bg_agent_extras().map(|e| e.retrieved).unwrap_or(false)
    }

    pub fn evict_after(&self) -> Option<i64> {
        self.bg_agent_extras().and_then(|e| e.evict_after)
    }
}

// ─── ID generation ─────────────────────────────────────────────────────

/// Generate a task id with the appropriate prefix for the variant.
pub fn generate_task_id(task_type: TaskType) -> String {
    match task_type {
        TaskType::BgAgent => generate_bg_agent_id(),
        TaskType::Shell => format!("b{}", random_alphanumeric(8)),
        TaskType::LocalWorkflow => format!("w{}", random_alphanumeric(8)),
        TaskType::Teammate => format!("t{}", random_alphanumeric(8)),
        TaskType::RemoteTeammate => format!("r{}", random_alphanumeric(8)),
        TaskType::Dream => format!("d{}", random_alphanumeric(8)),
    }
}

/// Generate the `a<16hex>` id shape used for backgrounded agent tasks.
/// Returned as `String` (the BgAgent's task id IS its agent id).
pub fn generate_bg_agent_id() -> String {
    let mut random = String::with_capacity(16);
    for _ in 0..8 {
        let byte = rand_u8();
        random.push(hex_digit(byte >> 4));
        random.push(hex_digit(byte & 0x0f));
    }
    format!("a{random}")
}

fn random_alphanumeric(n: usize) -> String {
    (0..n)
        .map(|_| {
            let idx = rand_u8() % 36;
            if idx < 10 {
                (b'0' + idx) as char
            } else {
                (b'a' + idx - 10) as char
            }
        })
        .collect()
}

fn hex_digit(nibble: u8) -> char {
    if nibble < 10 {
        (b'0' + nibble) as char
    } else {
        (b'a' + nibble - 10) as char
    }
}

fn rand_u8() -> u8 {
    uuid::Uuid::new_v4().as_bytes()[0]
}

// ─── FieldUpdate ───────────────────────────────────────────────────────

/// Three-state field-update primitive. Replaces the previous
/// `Option<Option<T>>` encoding (`Some(None)` = clear vs `None` = no
/// change) — opaque at call sites per the project's parameter-design
/// rule. Applies uniformly to `Option<T>` and required `T` slots.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum FieldUpdate<T> {
    #[default]
    Keep,
    Clear,
    Set(T),
}

impl<T> FieldUpdate<T> {
    /// Apply to an `Option<T>` slot. Returns true on change.
    pub fn apply(self, slot: &mut Option<T>) -> bool {
        match self {
            Self::Keep => false,
            Self::Clear => {
                let changed = slot.is_some();
                *slot = None;
                changed
            }
            Self::Set(v) => {
                *slot = Some(v);
                true
            }
        }
    }
}

impl<T: PartialEq + Default> FieldUpdate<T> {
    /// Apply to a non-`Option` slot. `Clear` resets to `T::default()`
    /// (false for `bool`, empty for `String`, etc). Returns true on
    /// change.
    pub fn apply_required(self, slot: &mut T) -> bool {
        match self {
            Self::Keep => false,
            Self::Clear => {
                let default = T::default();
                if *slot == default {
                    return false;
                }
                *slot = default;
                true
            }
            Self::Set(v) => {
                if *slot == v {
                    return false;
                }
                *slot = v;
                true
            }
        }
    }
}
