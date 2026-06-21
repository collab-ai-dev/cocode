//! In-process agent runner for teammate spawning and lifecycle management.
//!
//! Provides `InProcessAgentRunner` which creates isolated agent contexts,
//! runs queries, and manages completion/cancellation.
//!
//! ## Permission propagation
//!
//! In-process subagents inherit the leader's
//! [`coco_tool_runtime::ToolPermissionBridge`] via
//! `SessionRuntime::wire_engine` — SDK leaders get
//! [`coco_cli::sdk_server::SdkPermissionBridge`] (forwards over
//! `approval/askForApproval`); TUI leaders get the TUI bridge (P0
//! work). Cross-process pane teammates use
//! [`crate::runner_loop::MailboxPermissionBridge`] (mailbox file IPC).
//! There is no in-process mpsc bridge owned by this runner — that
//! circuit was orphaned after Phase D and removed in the cleanup pass.

use std::collections::HashMap;
use std::sync::Arc;

use coco_types::AgentIsolation;
use coco_types::MemoryScope;
use tokio::sync::RwLock;
use tokio::sync::oneshot;

// ── Agent Context ──

/// Isolated context for an in-process agent.
///
/// Each spawned agent gets its own context with an independent cancellation
/// signal and working directory.
#[derive(Debug, Clone)]
pub struct AgentContext {
    /// Unique identifier for this agent (format: "name@team").
    pub agent_id: String,
    /// Display name.
    pub agent_name: String,
    /// Team this agent belongs to.
    pub team_name: String,
    /// Optional UI color.
    pub color: Option<String>,
    /// Working directory for this agent.
    pub working_dir: String,
    /// Model override for this agent.
    pub model: Option<String>,
    /// System prompt override.
    pub system_prompt: Option<String>,
    /// Whether plan mode is required before implementing.
    pub plan_mode_required: bool,
    /// Whether this agent has been cancelled.
    pub cancelled: Arc<std::sync::atomic::AtomicBool>,
    /// Tools explicitly allowed without permission prompts.
    pub allowed_tools: Vec<String>,
    /// Whether unlisted tools can show permission prompts.
    pub allow_permission_prompts: bool,
    /// Thinking/effort level override.
    pub effort: Option<coco_types::ReasoningEffort>,
    /// Cache-identical tool schema prefixes.
    pub use_exact_tools: bool,
    /// Isolation mode for the execution environment.
    pub isolation: AgentIsolation,
    /// Memory persistence scope.
    pub memory_scope: Option<MemoryScope>,
    /// Per-agent MCP server names.
    pub mcp_servers: Vec<String>,
    /// Tools this agent is not allowed to use.
    pub disallowed_tools: Vec<String>,
    /// Maximum turns before the agent should stop.
    pub max_turns: Option<i32>,
}

impl AgentContext {
    /// Check if this agent has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Cancel this agent.
    pub fn cancel(&self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

// ── Spawn Configuration ──

/// Configuration for spawning an in-process teammate.
#[derive(Debug, Clone)]
pub struct SpawnConfig {
    /// Display name for the teammate.
    pub name: String,
    /// Team this teammate belongs to.
    pub team_name: String,
    /// Initial prompt/task for the teammate.
    pub prompt: String,
    /// Optional UI color.
    pub color: Option<String>,
    /// Whether teammate must enter plan mode before implementing.
    pub plan_mode_required: bool,
    /// Optional model override.
    pub model: Option<String>,
    /// Working directory (defaults to parent's CWD).
    pub working_dir: Option<String>,
    /// Optional system prompt override.
    pub system_prompt: Option<String>,
    /// Tools explicitly allowed without permission prompts.
    pub allowed_tools: Vec<String>,
    /// Whether unlisted tools can trigger permission prompts.
    pub allow_permission_prompts: bool,
    /// Thinking/effort level override.
    pub effort: Option<coco_types::ReasoningEffort>,
    /// Cache-identical tool schema prefixes.
    pub use_exact_tools: bool,
    /// Isolation mode for the execution environment.
    pub isolation: AgentIsolation,
    /// Memory persistence scope.
    pub memory_scope: Option<MemoryScope>,
    /// Per-agent MCP server names.
    pub mcp_servers: Vec<String>,
    /// Tools this agent is not allowed to use.
    pub disallowed_tools: Vec<String>,
    /// Maximum turns before the agent should stop.
    pub max_turns: Option<i32>,
}

// ── Spawn Result ──

/// Result from spawning an in-process teammate.
#[derive(Debug)]
pub struct SpawnResult {
    /// Whether spawn was successful.
    pub success: bool,
    /// Full agent ID (format: "name@team").
    pub agent_id: String,
    /// Error message if spawn failed.
    pub error: Option<String>,
}

// ── Runner Result ──

/// Result from running an in-process teammate to completion.
#[derive(Debug, Clone)]
pub struct RunnerResult {
    /// Whether the run completed successfully.
    pub success: bool,
    /// Error message if failed.
    pub error: Option<String>,
    /// Output text produced by the agent.
    pub output: Option<String>,
    /// Number of turns the agent executed.
    pub turns: i32,
}

// ── Agent Entry ──

/// Internal tracking entry for a spawned agent.
struct AgentEntry {
    context: AgentContext,
    /// Channel to receive the agent's result.
    result_rx: Option<oneshot::Receiver<RunnerResult>>,
}

// ── InProcessAgentRunner ──

/// In-process agent runner — spawns, tracks, and manages teammate agents.
///
/// Unlike process-based teammates (tmux/iTerm2), in-process teammates share
/// the same Tokio runtime but use independent cancellation flags and contexts
/// for isolation.
pub struct InProcessAgentRunner {
    /// Active agents keyed by agent_id.
    agents: Arc<RwLock<HashMap<String, AgentEntry>>>,
    /// Default working directory for spawned agents.
    default_working_dir: String,
    /// Maximum concurrent agents.
    max_agents: i32,
}

impl InProcessAgentRunner {
    /// Create a new runner.
    pub fn new(default_working_dir: String, max_agents: i32) -> Self {
        Self {
            agents: Arc::new(RwLock::new(HashMap::new())),
            default_working_dir,
            max_agents,
        }
    }

    /// Format an agent ID from name and team.
    fn format_agent_id(name: &str, team: &str) -> String {
        format!("{name}@{team}")
    }

    /// Register a new in-process agent (Phase 6).
    ///
    /// Creates an isolated `AgentContext` with its own cancellation flag
    /// and registers the agent. Returns a `SpawnResult` indicating success
    /// or failure.
    ///
    /// **Registration only** — execution must be started separately via
    /// [`Self::start_agent`]. Rust's tokio-spawn pattern needs an
    /// explicit registration step so the runner can wire a result
    /// channel before the execution task starts emitting output. Per
    /// the agent-loop refactor plan Phase 6, the old `spawn_agent`
    /// name implied "spawn = run", which silently succeeded even when
    /// the caller forgot to wire execution. The rename makes the
    /// split explicit.
    pub async fn register_agent(&self, config: SpawnConfig) -> SpawnResult {
        let agent_id = Self::format_agent_id(&config.name, &config.team_name);

        // Check capacity
        {
            let agents = self.agents.read().await;
            if agents.len() >= self.max_agents as usize {
                return SpawnResult {
                    success: false,
                    agent_id,
                    error: Some(format!(
                        "Max agents ({}) reached, cannot spawn '{}'",
                        self.max_agents, config.name
                    )),
                };
            }

            // Check for duplicate
            if agents.contains_key(&agent_id) {
                let msg = format!("Agent '{agent_id}' already exists");
                return SpawnResult {
                    success: false,
                    agent_id,
                    error: Some(msg),
                };
            }
        }

        let context = AgentContext {
            agent_id: agent_id.clone(),
            agent_name: config.name,
            team_name: config.team_name,
            color: config.color,
            working_dir: config
                .working_dir
                .unwrap_or_else(|| self.default_working_dir.clone()),
            model: config.model,
            system_prompt: config.system_prompt,
            plan_mode_required: config.plan_mode_required,
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            allowed_tools: config.allowed_tools,
            allow_permission_prompts: config.allow_permission_prompts,
            effort: config.effort,
            use_exact_tools: config.use_exact_tools,
            isolation: config.isolation,
            memory_scope: config.memory_scope,
            mcp_servers: config.mcp_servers,
            disallowed_tools: config.disallowed_tools,
            max_turns: config.max_turns,
        };

        let entry = AgentEntry {
            context,
            result_rx: None,
        };

        self.agents.write().await.insert(agent_id.clone(), entry);

        SpawnResult {
            success: true,
            agent_id,
            error: None,
        }
    }

    /// Wait for an agent to complete.
    ///
    /// Blocks until the agent finishes via its result channel. Returns `None`
    /// if the agent does not exist or has no result channel.
    pub async fn wait_for_completion(&self, agent_id: &str) -> Option<RunnerResult> {
        let rx = {
            let mut agents = self.agents.write().await;
            agents
                .get_mut(agent_id)
                .and_then(|entry| entry.result_rx.take())
        };

        match rx {
            Some(rx) => match rx.await {
                Ok(result) => Some(result),
                Err(_) => Some(RunnerResult {
                    success: false,
                    error: Some("Agent result channel closed unexpectedly".into()),
                    output: None,
                    turns: 0,
                }),
            },
            None => None,
        }
    }

    /// Cancel a running agent.
    ///
    /// Sets the agent's cancellation flag and removes it from the active
    /// agents map. Returns `true` if the agent was found and cancelled.
    pub async fn cancel_agent(&self, agent_id: &str) -> bool {
        let entry = self.agents.write().await.remove(agent_id);
        match entry {
            Some(entry) => {
                entry.context.cancel();
                true
            }
            None => false,
        }
    }

    /// Collect the result for an agent that has completed.
    ///
    /// Removes the agent from tracking and returns its result. Returns
    /// `None` if the agent doesn't exist.
    pub async fn collect_result(&self, agent_id: &str) -> Option<RunnerResult> {
        let entry = self.agents.write().await.remove(agent_id);
        match entry {
            Some(entry) => {
                if let Some(rx) = entry.result_rx {
                    match rx.await {
                        Ok(result) => Some(result),
                        Err(_) => Some(RunnerResult {
                            success: false,
                            error: Some("Agent result channel closed".into()),
                            output: None,
                            turns: 0,
                        }),
                    }
                } else {
                    Some(RunnerResult {
                        success: false,
                        error: Some("Agent did not produce a result".into()),
                        output: None,
                        turns: 0,
                    })
                }
            }
            None => None,
        }
    }

    /// Start the execution task for a registered agent (Phase 6).
    ///
    /// Takes ownership of a `JoinHandle<InProcessRunnerResult>` returned
    /// by [`crate::runner_loop::start_in_process_teammate`], spawns
    /// a forwarder task that translates the eventual join result into
    /// a `RunnerResult`, and atomically installs the oneshot receiver
    /// on the registered agent entry.
    ///
    /// Returns `true` if the agent was found and started; `false` if
    /// `agent_id` is not registered.
    ///
    /// **Why atomic**: the previous two-phase API (`set_result_channel`
    /// + separate `start_in_process_teammate`) let callers register an
    ///   agent and forget to wire the channel — `wait_for_completion`
    ///   would then silently return `None`. By taking the `JoinHandle`
    ///   directly, this method makes it a compile-time error to skip
    ///   the execution step.
    pub async fn start_agent(
        &self,
        agent_id: &str,
        handle: tokio::task::JoinHandle<crate::runner_loop::InProcessRunnerResult>,
    ) -> bool {
        let (tx, rx) = oneshot::channel::<RunnerResult>();

        // Forwarder: await the execution JoinHandle, map its result into
        // a RunnerResult, and deliver through the oneshot. If the task
        // panics or is cancelled, produce a descriptive RunnerResult
        // rather than propagating the JoinError.
        tokio::spawn(async move {
            let result = match handle.await {
                Ok(r) => RunnerResult {
                    success: r.success,
                    error: r.error,
                    output: r.output,
                    turns: r.turns,
                },
                Err(e) if e.is_cancelled() => RunnerResult {
                    success: false,
                    error: Some("Agent execution task was cancelled".into()),
                    output: None,
                    turns: 0,
                },
                Err(e) => RunnerResult {
                    success: false,
                    error: Some(format!("Agent execution task panicked: {e}")),
                    output: None,
                    turns: 0,
                },
            };
            // Best-effort delivery. If the receiver was dropped (caller
            // never awaited `wait_for_completion` / `collect_result`),
            // drop the result silently — it's already been observed via
            // task-state snapshots.
            let _ = tx.send(result);
        });

        let mut agents = self.agents.write().await;
        if let Some(entry) = agents.get_mut(agent_id) {
            entry.result_rx = Some(rx);
            true
        } else {
            false
        }
    }

    /// Get the agent context for an agent.
    pub async fn get_context(&self, agent_id: &str) -> Option<AgentContext> {
        self.agents
            .read()
            .await
            .get(agent_id)
            .map(|e| e.context.clone())
    }

    /// Get a list of all active (non-cancelled) agents.
    pub async fn active_agents(&self) -> Vec<AgentContext> {
        self.agents
            .read()
            .await
            .values()
            .filter(|e| !e.context.is_cancelled())
            .map(|e| e.context.clone())
            .collect()
    }

    /// Get the number of active agents.
    pub async fn active_count(&self) -> usize {
        self.agents
            .read()
            .await
            .values()
            .filter(|e| !e.context.is_cancelled())
            .count()
    }

    /// Cancel all active agents.
    pub async fn cancel_all(&self) {
        let agents = self.agents.write().await;
        for entry in agents.values() {
            entry.context.cancel();
        }
        drop(agents);
        self.agents.write().await.clear();
    }
}

#[cfg(test)]
#[path = "runner.test.rs"]
mod tests;
