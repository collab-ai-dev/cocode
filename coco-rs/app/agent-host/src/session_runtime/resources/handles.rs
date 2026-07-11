use std::{
    collections::HashMap,
    sync::{Arc, atomic::AtomicU64},
};

use coco_messages::MessageHistory;
use coco_tool_runtime::AgentHandleRef;
use tokio::sync::{Mutex, RwLock};

/// Shared handle to a `QueryEngine`'s post-turn cache-safe-params slot, as
/// returned by `QueryEngine::cache_safe_params_handle`. Kept as an alias so
/// the runtime field that stores the latest one stays readable.
type CacheParamsHandle = Arc<RwLock<Option<coco_types::CacheSafeParams>>>;

pub(in crate::session_runtime) struct SessionIntegrationResources {
    /// MCP handle installed on every per-turn engine via `wire_engine`.
    pub(in crate::session_runtime) mcp_handle: Arc<RwLock<Option<coco_tool_runtime::McpHandleRef>>>,
    /// Concrete MCP manager used by reload paths when this session owns one.
    pub(in crate::session_runtime) mcp_manager:
        Arc<RwLock<Option<Arc<tokio::sync::Mutex<coco_mcp::McpConnectionManager>>>>>,
    /// Monotonic "the MCP server set changed" signal.
    pub(in crate::session_runtime) mcp_reconnect_key: Arc<AtomicU64>,
    /// Late-bound LSP handle installed on every per-turn engine.
    pub(in crate::session_runtime) lsp_handle: Arc<RwLock<Option<coco_tool_runtime::LspHandleRef>>>,
    pub(in crate::session_runtime) reload_supervisor:
        Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    /// Last successful tool-registration outcome for each MCP server. This is
    /// session-owned because two live sessions may connect the same server
    /// name while exposing different tool catalogs.
    pub(in crate::session_runtime) mcp_registration_reports:
        Arc<RwLock<HashMap<String, crate::session_runtime::McpRegistrationStatus>>>,
}

impl SessionIntegrationResources {
    pub(in crate::session_runtime) fn new(
        mcp_handle: Arc<RwLock<Option<coco_tool_runtime::McpHandleRef>>>,
        mcp_manager: Arc<RwLock<Option<Arc<tokio::sync::Mutex<coco_mcp::McpConnectionManager>>>>>,
        mcp_reconnect_key: Arc<AtomicU64>,
        lsp_handle: Arc<RwLock<Option<coco_tool_runtime::LspHandleRef>>>,
        reload_supervisor: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    ) -> Self {
        Self {
            mcp_handle,
            mcp_manager,
            mcp_reconnect_key,
            lsp_handle,
            reload_supervisor,
            mcp_registration_reports: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub(in crate::session_runtime) fn mcp_handle(
        &self,
    ) -> &Arc<RwLock<Option<coco_tool_runtime::McpHandleRef>>> {
        &self.mcp_handle
    }

    pub(in crate::session_runtime) fn mcp_manager(
        &self,
    ) -> &Arc<RwLock<Option<Arc<tokio::sync::Mutex<coco_mcp::McpConnectionManager>>>>> {
        &self.mcp_manager
    }

    pub(in crate::session_runtime) fn mcp_reconnect_key(&self) -> &Arc<AtomicU64> {
        &self.mcp_reconnect_key
    }

    pub(in crate::session_runtime) fn lsp_handle(
        &self,
    ) -> &Arc<RwLock<Option<coco_tool_runtime::LspHandleRef>>> {
        &self.lsp_handle
    }

    pub(in crate::session_runtime) fn reload_supervisor(
        &self,
    ) -> &Arc<Mutex<Option<tokio::task::JoinHandle<()>>>> {
        &self.reload_supervisor
    }

    pub(in crate::session_runtime) fn mcp_registration_reports(
        &self,
    ) -> &Arc<RwLock<HashMap<String, crate::session_runtime::McpRegistrationStatus>>> {
        &self.mcp_registration_reports
    }
}

#[derive(Clone)]
pub(in crate::session_runtime) struct SessionHandleResources {
    /// Eager no-op-capable agent handle installed on every engine before the
    /// concrete coordinator-backed handle is late-bound.
    pub(in crate::session_runtime) swarm_agent_handle: coco_tool_runtime::AgentHandleRef,
    /// Real `AgentHandle` for `AgentTool` calls and forked subagents.
    pub(in crate::session_runtime) agent_handle: Arc<RwLock<Option<AgentHandleRef>>>,
    /// Skill-execution handle (`QuerySkillRuntime`), late-bound with the
    /// concrete agent handle because both wrap the subagent engine factory.
    pub(in crate::session_runtime) skill_handle:
        Arc<RwLock<Option<coco_tool_runtime::SkillHandleRef>>>,
    /// Shared Bash handle used by model-invoked and fork-mode skill paths.
    pub(in crate::session_runtime) skill_bash_cell:
        Arc<std::sync::RwLock<Option<Arc<dyn coco_skills::shell_exec::BashToolHandle>>>>,
    /// Post-turn fork dispatcher installed after the runtime Arc exists.
    pub(in crate::session_runtime) fork_dispatcher:
        Arc<RwLock<Option<coco_query::forked_agent::ForkDispatcherRef>>>,
    /// Latest per-turn cache-safe-params slot captured from built engines.
    pub(in crate::session_runtime) last_engine_cache_handle: Arc<RwLock<Option<CacheParamsHandle>>>,
    /// Session-scoped abort token for the in-flight prompt-suggestion fork.
    pub(in crate::session_runtime) current_suggestion_abort:
        Arc<tokio::sync::Mutex<Option<tokio_util::sync::CancellationToken>>>,
    /// Background task runtime shared with the agent handle.
    pub(in crate::session_runtime) task_runtime:
        Arc<RwLock<Option<Arc<crate::task_runtime::TaskRuntime>>>>,
    /// Durable task-list store shared by leader and agent children.
    pub(in crate::session_runtime) task_list:
        Arc<RwLock<Option<coco_tool_runtime::TaskListHandleRef>>>,
    pub(in crate::session_runtime) team_task_list_router:
        Arc<RwLock<Option<coco_tool_runtime::TeamTaskListRouterRef>>>,
    /// Session-scoped V1 TodoWrite store.
    pub(in crate::session_runtime) todo_list: Arc<RwLock<coco_tool_runtime::TodoListHandleRef>>,
    /// Per-agent transcript / metadata store for resume support.
    pub(in crate::session_runtime) agent_transcript_store:
        Arc<RwLock<Option<coco_tool_runtime::AgentTranscriptStoreRef>>>,
}

impl SessionHandleResources {
    pub(in crate::session_runtime) fn new(
        swarm_agent_handle: coco_tool_runtime::AgentHandleRef,
    ) -> Self {
        Self {
            swarm_agent_handle,
            agent_handle: Arc::new(RwLock::new(None)),
            skill_handle: Arc::new(RwLock::new(None)),
            skill_bash_cell: Arc::new(std::sync::RwLock::new(None)),
            fork_dispatcher: Arc::new(RwLock::new(None)),
            last_engine_cache_handle: Arc::new(RwLock::new(None)),
            current_suggestion_abort: Arc::new(tokio::sync::Mutex::new(None)),
            task_runtime: Arc::new(RwLock::new(None)),
            task_list: Arc::new(RwLock::new(None)),
            team_task_list_router: Arc::new(RwLock::new(None)),
            todo_list: Arc::new(RwLock::new(Arc::new(
                coco_tool_runtime::InMemoryTodoListHandle::new(),
            ))),
            agent_transcript_store: Arc::new(RwLock::new(None)),
        }
    }
}

#[derive(Clone)]
pub(in crate::session_runtime) struct SessionPermissionResources {
    /// Teammate-scoped live permission-rule overlay injected into each main
    /// session engine config.
    pub(in crate::session_runtime) live_permission_rules:
        Arc<RwLock<Vec<coco_types::PermissionRule>>>,
}

impl SessionPermissionResources {
    pub(in crate::session_runtime) fn new() -> Self {
        Self {
            live_permission_rules: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

impl Default for SessionPermissionResources {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub(in crate::session_runtime) struct SessionAgentCatalogResources {
    /// Where the agent loader looks for markdown agents. Plugin reload
    /// refreshes this from the latest project-services snapshot.
    pub(in crate::session_runtime) agent_search_paths:
        Arc<RwLock<coco_subagent::definition_store::AgentSearchPaths>>,
    /// Built-in agent toggles applied to every reload.
    pub(in crate::session_runtime) builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog,
    /// Active per-session agent catalog snapshot.
    pub(in crate::session_runtime) agent_catalog:
        Arc<RwLock<Arc<coco_subagent::AgentCatalogSnapshot>>>,
    /// SDK-supplied agent definitions injected into every fresh catalog load.
    pub(in crate::session_runtime) sdk_supplied_agents:
        Arc<RwLock<Vec<coco_types::AgentDefinition>>>,
}

impl SessionAgentCatalogResources {
    pub(in crate::session_runtime) fn new(
        agent_search_paths: coco_subagent::definition_store::AgentSearchPaths,
        builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog,
        agent_catalog: Arc<RwLock<Arc<coco_subagent::AgentCatalogSnapshot>>>,
    ) -> Self {
        Self {
            agent_search_paths: Arc::new(RwLock::new(agent_search_paths)),
            builtin_agent_catalog,
            agent_catalog,
            sdk_supplied_agents: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

#[derive(Clone)]
pub(in crate::session_runtime) struct SessionMemoryResources {
    /// Auto-memory runtime — extraction / dream / session memory / recall
    /// ranker. `None` when `Feature::AutoMemory` is off.
    pub(in crate::session_runtime) memory_runtime: Option<Arc<coco_memory::MemoryRuntime>>,
    /// Skill-learning review runtime. `Some` when `Feature::SkillLearning`
    /// is enabled.
    pub(in crate::session_runtime) skill_review_runtime:
        Option<Arc<coco_skill_learn::SkillReviewRuntime>>,
}

impl SessionMemoryResources {
    pub(in crate::session_runtime) fn new(
        memory_runtime: Option<Arc<coco_memory::MemoryRuntime>>,
        skill_review_runtime: Option<Arc<coco_skill_learn::SkillReviewRuntime>>,
    ) -> Self {
        Self {
            memory_runtime,
            skill_review_runtime,
        }
    }
}

#[derive(Clone)]
pub(in crate::session_runtime) struct SessionSandboxResources {
    /// Session-scoped sandbox state shared by engines, SDK controls, and fork
    /// dispatch.
    pub(in crate::session_runtime) sandbox_state: Option<Arc<coco_sandbox::SandboxState>>,
}

impl SessionSandboxResources {
    pub(in crate::session_runtime) fn new(
        sandbox_state: Option<Arc<coco_sandbox::SandboxState>>,
    ) -> Self {
        Self { sandbox_state }
    }
}

#[derive(Clone)]
pub(in crate::session_runtime) struct SessionHistoryResources {
    /// Multi-turn agent transcript shared across per-turn engines and runtime
    /// control paths.
    pub(in crate::session_runtime) history: Arc<Mutex<MessageHistory>>,
}

impl SessionHistoryResources {
    pub(in crate::session_runtime) fn new(history: Arc<Mutex<MessageHistory>>) -> Self {
        Self { history }
    }

    pub(in crate::session_runtime) fn history(&self) -> &Arc<Mutex<MessageHistory>> {
        &self.history
    }
}
