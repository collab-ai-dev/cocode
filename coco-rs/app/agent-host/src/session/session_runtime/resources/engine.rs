use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use coco_context::FileHistoryState;
use coco_context::FileReadState;
use coco_messages::Message;
use coco_query::QueryEngineConfig;
use coco_types::ModelRole;
use coco_types::SessionId;
use coco_types::ToolAppState;
use tokio::sync::Mutex;
use tokio::sync::RwLock;

use crate::session_runtime::RoleOverride;

pub(in crate::session::session_runtime) struct SessionWorkspaceResources {
    /// Original CWD captured at session start. Frozen for the lifetime
    /// of this [`crate::session_runtime::SessionRuntime`] - never moves even if the user
    /// `cd`'s away inside a Bash command. Used as the anchor for
    /// `reset_cwd_if_outside_project` (when bash drifts out of the
    /// allowed working directory set, we snap it back here) and for
    /// "Shell cwd was reset to ..." stderr annotations.
    pub(in crate::session::session_runtime) original_cwd: PathBuf,
    /// Git worktree root for project-scoped services, or
    /// [`Self::original_cwd`] when the session is outside git.
    pub(in crate::session::session_runtime) project_root: PathBuf,
    /// Currently active CWD. Updated across BashTool calls so the
    /// model's `cd /tmp` in one turn survives into the next turn.
    /// Threaded into every `ToolUseContext` via the engine config so
    /// BashTool can read it as the spawn cwd and write back from
    /// `CommandResult.new_cwd`.
    pub(in crate::session::session_runtime) current_cwd: Arc<RwLock<PathBuf>>,
}

impl SessionWorkspaceResources {
    pub(in crate::session::session_runtime) fn new(
        original_cwd: PathBuf,
        project_root: PathBuf,
        current_cwd: Arc<RwLock<PathBuf>>,
    ) -> Self {
        Self {
            original_cwd,
            project_root,
            current_cwd,
        }
    }

    pub(in crate::session::session_runtime) fn original_cwd(&self) -> &PathBuf {
        &self.original_cwd
    }

    pub(in crate::session::session_runtime) fn project_root(&self) -> &PathBuf {
        &self.project_root
    }

    pub(in crate::session::session_runtime) fn current_cwd(&self) -> &Arc<RwLock<PathBuf>> {
        &self.current_cwd
    }
}

#[derive(Clone)]
pub(in crate::session::session_runtime) struct SessionEngineConfigResources {
    /// Immutable session identity for this runtime. Owned here rather than
    /// inside `engine_config` so per-turn config edits can never rotate the
    /// session id (which would split-brain the `SessionHandle` snapshot, the
    /// seq allocator's per-session domain, and the persisted transcript).
    pub(in crate::session::session_runtime) session_id: SessionId,
    /// Engine config; mutated by [`crate::session_runtime::SessionRuntime::update_engine_config`] and
    /// read by every per-turn build.
    pub(in crate::session::session_runtime) engine_config: Arc<RwLock<QueryEngineConfig>>,
    /// Synchronous snapshot for detached hook factories. Those
    /// factories run from async tasks but expose a sync `Fn()`, so they
    /// must not call Tokio `blocking_read()` on runtime worker threads.
    pub(in crate::session::session_runtime) orchestration_engine_config:
        Arc<std::sync::RwLock<QueryEngineConfig>>,
    /// Per-session in-memory model-role overrides. Populated by the TUI
    /// model picker (`UserCommand::SetModelRole`) and Ctrl+T thinking
    /// cycle (`UserCommand::SetThinkingLevel`). Layered above
    /// `runtime_config.model_roles`.
    pub(in crate::session::session_runtime) role_overrides:
        Arc<RwLock<HashMap<ModelRole, RoleOverride>>>,
}

impl SessionEngineConfigResources {
    pub(in crate::session::session_runtime) fn new(
        session_id: SessionId,
        engine_config: Arc<RwLock<QueryEngineConfig>>,
        orchestration_engine_config: Arc<std::sync::RwLock<QueryEngineConfig>>,
        role_overrides: Arc<RwLock<HashMap<ModelRole, RoleOverride>>>,
    ) -> Self {
        Self {
            session_id,
            engine_config,
            orchestration_engine_config,
            role_overrides,
        }
    }

    pub(in crate::session::session_runtime) fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub(in crate::session::session_runtime) fn engine_config(
        &self,
    ) -> &Arc<RwLock<QueryEngineConfig>> {
        &self.engine_config
    }

    pub(in crate::session::session_runtime) fn orchestration_engine_config(
        &self,
    ) -> &Arc<std::sync::RwLock<QueryEngineConfig>> {
        &self.orchestration_engine_config
    }

    pub(in crate::session::session_runtime) fn role_overrides(
        &self,
    ) -> &Arc<RwLock<HashMap<ModelRole, RoleOverride>>> {
        &self.role_overrides
    }
}

#[derive(Clone)]
pub(in crate::session::session_runtime) struct SessionEngineStateResources {
    pub(in crate::session::session_runtime) file_read_state: Arc<RwLock<FileReadState>>,
    pub(in crate::session::session_runtime) file_history: Option<Arc<RwLock<FileHistoryState>>>,
    pub(in crate::session::session_runtime) app_state: Arc<RwLock<ToolAppState>>,
    /// Session-scoped `/env` overrides consumed by shell providers before
    /// every child shell spawn. `None` when shell tools are disabled.
    pub(in crate::session::session_runtime) session_env_vars: Option<coco_shell::SessionEnvVars>,
    /// `/loop` scheduled sentinel memory. Reset after compaction so the next
    /// sentinel delivery re-establishes full instructions in the transcript.
    pub(in crate::session::session_runtime) loop_sentinel_state:
        Arc<Mutex<coco_skills::bundled::loop_skill::LoopSentinelState>>,
    /// Session-scoped peer-message store shared by every per-turn engine.
    pub(in crate::session::session_runtime) pending_message_store:
        coco_tool_runtime::PendingMessageStoreRef,
    /// Session-scoped Auto mode classifier state.
    pub(in crate::session::session_runtime) auto_mode_state: Arc<coco_permissions::AutoModeState>,
    /// Denial history for Auto mode classifier decisions.
    pub(in crate::session::session_runtime) denial_tracker:
        Arc<tokio::sync::Mutex<coco_permissions::DenialTracker>>,
    /// Cross-engine dedup set of message UUIDs already persisted to JSONL.
    pub(in crate::session::session_runtime) transcript_dedup:
        Arc<tokio::sync::Mutex<std::collections::HashSet<uuid::Uuid>>>,
    /// Conversation snapshot captured immediately before `/clear`.
    pub(in crate::session::session_runtime) clear_rewind_messages:
        Arc<tokio::sync::Mutex<Option<Vec<Arc<Message>>>>>,
    /// Cross-engine tool-result replacement state.
    pub(in crate::session::session_runtime) tool_result_replacement_state:
        coco_tool_runtime::tool_result_offload::ContentReplacementStateRef,
}

impl SessionEngineStateResources {
    #[allow(clippy::too_many_arguments)]
    pub(in crate::session::session_runtime) fn new(
        file_read_state: Arc<RwLock<FileReadState>>,
        file_history: Option<Arc<RwLock<FileHistoryState>>>,
        app_state: Arc<RwLock<ToolAppState>>,
        session_env_vars: Option<coco_shell::SessionEnvVars>,
        loop_sentinel_state: Arc<Mutex<coco_skills::bundled::loop_skill::LoopSentinelState>>,
        pending_message_store: coco_tool_runtime::PendingMessageStoreRef,
        auto_mode_state: Arc<coco_permissions::AutoModeState>,
        denial_tracker: Arc<tokio::sync::Mutex<coco_permissions::DenialTracker>>,
        transcript_dedup: Arc<tokio::sync::Mutex<std::collections::HashSet<uuid::Uuid>>>,
        clear_rewind_messages: Arc<tokio::sync::Mutex<Option<Vec<Arc<Message>>>>>,
        tool_result_replacement_state: coco_tool_runtime::tool_result_offload::ContentReplacementStateRef,
    ) -> Self {
        Self {
            file_read_state,
            file_history,
            app_state,
            session_env_vars,
            loop_sentinel_state,
            pending_message_store,
            auto_mode_state,
            denial_tracker,
            transcript_dedup,
            clear_rewind_messages,
            tool_result_replacement_state,
        }
    }

    pub(in crate::session::session_runtime) fn file_read_state(
        &self,
    ) -> &Arc<RwLock<FileReadState>> {
        &self.file_read_state
    }

    pub(in crate::session::session_runtime) fn file_history(
        &self,
    ) -> Option<&Arc<RwLock<FileHistoryState>>> {
        self.file_history.as_ref()
    }

    pub(in crate::session::session_runtime) fn app_state(&self) -> &Arc<RwLock<ToolAppState>> {
        &self.app_state
    }

    pub(in crate::session::session_runtime) fn session_env_vars(
        &self,
    ) -> Option<&coco_shell::SessionEnvVars> {
        self.session_env_vars.as_ref()
    }

    pub(in crate::session::session_runtime) fn loop_sentinel_state(
        &self,
    ) -> &Arc<Mutex<coco_skills::bundled::loop_skill::LoopSentinelState>> {
        &self.loop_sentinel_state
    }

    pub(in crate::session::session_runtime) fn pending_message_store(
        &self,
    ) -> &coco_tool_runtime::PendingMessageStoreRef {
        &self.pending_message_store
    }

    pub(in crate::session::session_runtime) fn auto_mode_state(
        &self,
    ) -> &Arc<coco_permissions::AutoModeState> {
        &self.auto_mode_state
    }

    pub(in crate::session::session_runtime) fn denial_tracker(
        &self,
    ) -> &Arc<tokio::sync::Mutex<coco_permissions::DenialTracker>> {
        &self.denial_tracker
    }

    pub(in crate::session::session_runtime) fn transcript_dedup(
        &self,
    ) -> &Arc<tokio::sync::Mutex<std::collections::HashSet<uuid::Uuid>>> {
        &self.transcript_dedup
    }

    pub(in crate::session::session_runtime) fn clear_rewind_messages(
        &self,
    ) -> &Arc<tokio::sync::Mutex<Option<Vec<Arc<Message>>>>> {
        &self.clear_rewind_messages
    }

    pub(in crate::session::session_runtime) fn tool_result_replacement_state(
        &self,
    ) -> &coco_tool_runtime::tool_result_offload::ContentReplacementStateRef {
        &self.tool_result_replacement_state
    }
}
