//! In-process teammate execution loop.
//!
//! Manages the multi-turn agent loop for in-process teammates:
//! prompt → run query → idle → wait for next prompt → loop.
//!
//! The actual LLM query execution is abstracted behind the
//! [`AgentExecutionEngine`] trait, allowing `app/query` to provide
//! the implementation without circular dependencies.
//!
//! ## Module split (P1)
//!
//! Helper functions live in sibling modules to keep this file under
//! the 800-LoC cap:
//!
//! - [`crate::runner_loop_mailbox_permission`] — cross-process
//!   permission via mailbox IPC + `MailboxPermissionBridge` impl.
//! - [`crate::runner_loop_wait`] — `wait_for_plan_approval`.
//! - [`crate::runner_loop_notify`] — outbound notification helpers
//!   (`send_message_to_leader`, `send_idle_notification`,
//!   `format_task_as_prompt`, `find_available_task`).

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

use async_trait::async_trait;
use coco_messages::Message;
use coco_tool_runtime::AgentTaskRegistryRef;
use coco_tool_runtime::TaskListHandleRef;
use coco_tool_runtime::TeammateTaskUpdate;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::constants::TEAM_LEAD_NAME;
use crate::mailbox;
use crate::pane::SystemPromptMode;
use crate::prompt;
use crate::roster_store::SetMemberActiveRequest;
use crate::roster_store::TeamRosterStore;
use crate::teammate;
use crate::types::TeammateIdentity;

// ── Trait: AgentExecutionEngine ──

/// Configuration for a single query/turn within the teammate loop.
#[derive(Debug, Clone, Default)]
pub struct AgentQueryConfig {
    /// System prompt (base + addendum).
    pub system_prompt: String,
    /// Model override.
    pub model: Option<String>,
    /// Maximum turns for this query.
    pub max_turns: Option<i32>,
    /// Tools the agent is allowed to use without prompting.
    pub allowed_tools: Vec<String>,
    /// Tools explicitly denied to the agent regardless of allow-list.
    /// Sourced from the role's `disallowed_tools`. Threaded onward into
    /// the engine-side `coco_tool_runtime::AgentQueryConfig` so Layer 4
    /// of the filter pipeline narrows correctly.
    pub disallowed_tools: Vec<String>,
    /// Prior conversation messages (for context forking). Shared
    /// `Arc<Message>` so each teammate turn re-uses the same
    /// allocations across the loop — no serialize / deserialize hop.
    pub fork_context_messages: Vec<Arc<Message>>,
    /// Whether to preserve full tool results (not previews).
    pub preserve_tool_use_results: bool,
    /// Parent session's bypass-permissions capability. Forwarded to the
    /// teammate's `ToolPermissionContext.bypass_available` so in-process
    /// teammates observe the same cycle + plan-exit gate as the leader.
    pub bypass_permissions_available: bool,
    /// Parent session's resolved Layer 1 features. The engine bridge
    /// must thread this into `coco_tool_runtime::AgentQueryConfig.features`
    /// or teammates silently get registry defaults instead of the user's
    /// runtime resolution.
    pub features: Option<std::sync::Arc<coco_types::Features>>,
    /// Parent session's resolved Layer 2 tool overrides. Same rationale
    /// as `features`: a `None` here causes the engine bridge to fall
    /// back to `ToolOverrides::none()`, widening the set the active
    /// model accepts.
    pub tool_overrides: Option<std::sync::Arc<coco_types::ToolOverrides>>,
    /// Parent session's Layer 4 tool filter. The engine bridge feeds
    /// this into `coco_tool_runtime::AgentQueryConfig.parent_tool_filter`
    /// so the teammate's own allow/deny gets intersected via
    /// `ToolFilter::narrow_with`.
    pub parent_tool_filter: Option<coco_types::ToolFilter>,
    /// Parent session's resolved shell tool visibility.
    pub active_shell_tool: coco_types::ActiveShellTool,
    pub effort: Option<coco_types::ReasoningEffort>,
    pub use_exact_tools: bool,
    pub mcp_servers: Vec<String>,
    pub model_role: Option<coco_types::ModelRole>,
    pub model_selection: coco_types::LlmModelSelection,
    pub permission_mode: Option<coco_types::PermissionMode>,
    pub extra_permission_rules: Vec<coco_types::PermissionRule>,
    pub live_permission_rules: Option<Arc<RwLock<Vec<coco_types::PermissionRule>>>>,
    pub live_permission_mode: Option<Arc<RwLock<coco_types::PermissionMode>>>,
    pub cancel: Option<CancellationToken>,
    pub session_id: String,
    pub agent_id: String,
}

/// Result from running a single query/turn.
#[derive(Debug, Clone)]
pub struct AgentQueryResult {
    /// Messages produced during this query. Shared `Arc<Message>` so
    /// the runner_loop's accumulator extends without deep-cloning
    /// every entry per turn.
    pub messages: Vec<Arc<Message>>,
    /// Total token count for this query.
    pub token_count: i64,
    /// Input tokens used.
    pub input_tokens: i64,
    /// Output tokens produced.
    pub output_tokens: i64,
    /// Number of turns executed.
    pub turns: i32,
    /// Number of tool uses.
    pub tool_use_count: i32,
    /// Whether the query was cancelled.
    pub cancelled: bool,
    /// Response text (last assistant message).
    pub response_text: Option<String>,
}

/// Abstraction over the query engine for teammate execution.
///
/// Implemented by `app/query` to avoid circular dependencies.
/// The runner loop calls `run_query()` for each turn of the
/// teammate's conversation.
#[async_trait]
pub trait AgentExecutionEngine: Send + Sync {
    /// Run a single agent query (one or more LLM turns).
    async fn run_query(
        &self,
        prompt: &str,
        config: AgentQueryConfig,
    ) -> crate::Result<AgentQueryResult>;

    /// Semantically compact the worker's message history when it grows
    /// past the auto-compact threshold. Implementers route through
    /// `coco-compact` to either run a full LLM compaction (replaces
    /// older turns with a summary) or a micro-compact (drops resolved
    /// tool results). The default returns the input unchanged so test
    /// engines and engines without compaction support don't have to
    /// implement it.
    ///
    /// Called by [`run_in_process_teammate`] at the tail of each turn
    /// when token usage exceeds [`InProcessRunnerConfig::auto_compact_threshold`].
    async fn compact_messages(
        &self,
        messages: Vec<Arc<Message>>,
        _total_tokens: i64,
        // Parent session + teammate agent id — used by real engines to
        // scope the summarization sub-run's identity. Ignored by the
        // no-op default.
        _session_id: &str,
        _agent_id: &str,
    ) -> crate::Result<Vec<Arc<Message>>> {
        // No-op default: keep the input. Real engines should override.
        Ok(messages)
    }
}

// ── Runner Config & Result ──

/// Configuration for running an in-process teammate.
#[derive(Clone)]
pub struct InProcessRunnerConfig {
    /// Teammate identity.
    pub identity: TeammateIdentity,
    /// Task ID in AppState.
    pub task_id: String,
    /// Parent session id shared by this teammate run.
    pub session_id: String,
    /// Initial prompt/task.
    pub prompt: String,
    /// Model override.
    pub model: Option<String>,
    /// Base system prompt.
    pub system_prompt: Option<String>,
    /// System prompt assembly mode.
    pub system_prompt_mode: SystemPromptMode,
    /// Tools allowed without permission prompts.
    pub allowed_tools: Vec<String>,
    /// Whether to allow permission prompts for unlisted tools.
    pub allow_permission_prompts: bool,
    /// Maximum turns per query.
    pub max_turns: Option<i32>,
    /// Cancellation flag (shared with AgentContext).
    pub cancelled: Arc<AtomicBool>,
    /// Auto-compact threshold (token count).
    pub auto_compact_threshold: i64,
    /// Parent session's bypass-permissions capability (forwarded on
    /// every query). Defaults to `false` so legacy callers stay safe.
    pub bypass_permissions_available: bool,
    /// Parent session's resolved Layer 1 features. Threaded into every
    /// teammate query config so in-process teammates see the same gate
    /// set as the leader.
    pub features: Option<std::sync::Arc<coco_types::Features>>,
    /// Parent session's resolved Layer 2 tool overrides. Same rationale.
    pub tool_overrides: Option<std::sync::Arc<coco_types::ToolOverrides>>,
    /// Parent session's Layer 4 tool filter — see `AgentQueryConfig`.
    pub parent_tool_filter: Option<coco_types::ToolFilter>,
    /// Parent session's resolved shell tool visibility.
    pub active_shell_tool: coco_types::ActiveShellTool,
    pub effort: Option<coco_types::ReasoningEffort>,
    pub use_exact_tools: bool,
    pub mcp_servers: Vec<String>,
    pub disallowed_tools: Vec<String>,
    pub model_role: Option<coco_types::ModelRole>,
    pub model_selection: coco_types::LlmModelSelection,
    pub task_list: Option<TaskListHandleRef>,
    pub task_registry: Option<AgentTaskRegistryRef>,
    pub roster_store: Option<TeamRosterStore>,
    /// Whether the leader must approve the teammate's plan before any
    /// implementation turn runs. When `true`, after the first turn (the
    /// plan-write turn) the runner sends a
    /// [`mailbox::ProtocolMessage::PlanApprovalRequest`] to the leader
    /// and blocks via [`wait_for_plan_approval`] until a matching
    /// [`mailbox::ProtocolMessage::PlanApprovalResponse`] arrives. On
    /// rejection the loop continues with the leader's feedback as the
    /// next prompt; on approval the gate drops permanently for the
    /// remainder of the session.
    pub plan_mode_required: bool,
    /// Optional hook registry + orchestration context for firing the
    /// `TeammateIdle` event when the teammate transitions to idle.
    /// When the hook returns blocking, the teammate stays in working state and
    /// receives the hook's feedback as the next prompt instead of
    /// going idle. `None` means no hook firing (legacy / tests).
    pub hooks: Option<std::sync::Arc<coco_hooks::HookRegistry>>,
    /// Hook orchestration context, paired with `hooks`. Cloned for
    /// each TeammateIdle firing.
    pub orchestration_ctx: Option<coco_hooks::orchestration::OrchestrationContext>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TeammateControlState {
    permission_mode: Arc<RwLock<coco_types::PermissionMode>>,
    team_permission_rules: Arc<RwLock<Vec<coco_types::PermissionRule>>>,
}

/// Result from running an in-process teammate to completion.
#[derive(Debug, Clone)]
pub struct InProcessRunnerResult {
    pub success: bool,
    pub error: Option<String>,
    pub output: Option<String>,
    pub turns: i32,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
}

// ── Wait Result ──

/// Result from waiting for the next prompt or shutdown.
#[derive(Debug, Clone)]
pub enum WaitResult {
    /// Shutdown requested by leader.
    ShutdownRequest { original_text: String },
    /// New message received (from leader, peer, or task list).
    NewMessage {
        message: String,
        from: String,
        color: Option<String>,
        summary: Option<String>,
    },
    /// Agent was aborted (lifecycle cancellation).
    Aborted,
}

// ── Main Execution Loop ──

/// Run an in-process teammate to completion.
///
/// Flow:
/// 1. Build system prompt (base + addendum per mode)
/// 2. Try to claim an initial task from the task list
/// 3. Loop: run query → transition to idle → wait for next prompt
/// 4. Handle compaction when token count exceeds threshold
/// 5. Update task state at each lifecycle phase
/// 6. Send idle notification on idle transition
/// 7. Cleanup on exit (completion, failure, or abort)
pub async fn run_in_process_teammate(
    config: InProcessRunnerConfig,
    engine: &dyn AgentExecutionEngine,
) -> InProcessRunnerResult {
    // Build system prompt
    let system_prompt = prompt::build_teammate_system_prompt(
        config.system_prompt.as_deref(),
        None,
        config.system_prompt_mode,
    );

    let mut all_messages: Vec<Arc<Message>> = Vec::new();
    let mut current_prompt = teammate::format_as_teammate_message(
        TEAM_LEAD_NAME,
        &config.prompt,
        None,
        Some("initial prompt"),
    );
    let initial_permission_mode = if config.plan_mode_required {
        coco_types::PermissionMode::Plan
    } else {
        coco_types::PermissionMode::Default
    };
    let live_permission_mode = Arc::new(RwLock::new(initial_permission_mode));
    let live_permission_rules = Arc::new(RwLock::new(load_team_allowed_path_rules(
        &config.identity.team_name,
    )));
    let control_state = RwLock::new(TeammateControlState {
        permission_mode: live_permission_mode.clone(),
        team_permission_rules: live_permission_rules.clone(),
    });
    update_teammate_task(
        &config,
        TeammateTaskUpdate {
            append_message: Some(coco_types::TeammateTaskMessage {
                role: coco_types::MessageRole::User,
                content: current_prompt.clone(),
                tool_name: None,
            }),
            ..TeammateTaskUpdate::default()
        },
    )
    .await;
    let mut total_turns = 0i32;
    let mut total_input_tokens = 0i64;
    let mut total_output_tokens = 0i64;
    let mut total_tool_use_count: i32 = 0;
    // Cap-3 ring buffer of recent tool names for the teammate spinner
    // tree preview. Capped at the same limit as the renderer's preview.
    let mut recent_activities: std::collections::VecDeque<coco_types::TaskActivity> =
        std::collections::VecDeque::with_capacity(3);
    let mut last_tool_name: Option<String> = None;
    let mut was_idle = false;
    // `Some(msg)` once a query failure has been observed — the unified
    // cleanup path at the bottom uses this to flip the
    // `<task-notification>` status to `Failed` and the
    // `on_teammate_stop` reason to "failed: {msg}".
    let mut run_error: Option<String> = None;
    // Shutdown is decided by the MODEL, not the runner: a leader
    // `ShutdownRequest` is delivered as an ordinary turn, and the loop
    // exits only if the model APPROVES — its `shutdown_response` tool call
    // runs `signal_self_stop`, flipping `config.cancelled` so the
    // `config.cancelled` check below breaks. A rejection leaves the flag
    // clear and the teammate keeps working.
    // Plan-approval gate — initially open iff the spawn did NOT request
    // plan-mode. When `true`, the runner suspends after each model turn
    // and pushes a `PlanApprovalRequest` to the leader, then awaits the
    // matching response via `wait_for_plan_approval` before continuing.
    // Once approved (or once the spawn never asked for plan-mode) the
    // flag stays `false` for the rest of the session. Only gates between
    // the plan-write turn and the first implementation turn — the flag is
    // dropped on the first approval rather than re-arming.
    let mut plan_approval_pending = config.plan_mode_required;

    // Main loop
    loop {
        // Check cancellation
        if config.cancelled.load(Ordering::Relaxed) {
            break;
        }

        // Update task: mark as running
        update_teammate_task(
            &config,
            TeammateTaskUpdate {
                is_idle: coco_types::FieldUpdate::Set(false),
                spinner_verb: coco_types::FieldUpdate::Set("Working".to_string()),
                ..TeammateTaskUpdate::default()
            },
        )
        .await;

        let mut last_turn_interrupted = false;
        let current_turn_cancel = CancellationToken::new();
        let (permission_mode, live_permission_mode, live_permission_rules) = {
            let state = control_state.read().await;
            (
                *state.permission_mode.read().await,
                state.permission_mode.clone(),
                state.team_permission_rules.clone(),
            )
        };
        set_teammate_current_work_cancel(&config, Some(current_turn_cancel.clone())).await;
        // Build query config
        let query_config = AgentQueryConfig {
            system_prompt: system_prompt.clone(),
            model: config.model.clone(),
            max_turns: config.max_turns,
            allowed_tools: config.allowed_tools.clone(),
            disallowed_tools: config.disallowed_tools.clone(),
            fork_context_messages: all_messages.clone(),
            preserve_tool_use_results: true,
            bypass_permissions_available: config.bypass_permissions_available,
            features: config.features.clone(),
            tool_overrides: config.tool_overrides.clone(),
            parent_tool_filter: config.parent_tool_filter.clone(),
            active_shell_tool: config.active_shell_tool,
            effort: config.effort,
            use_exact_tools: config.use_exact_tools,
            mcp_servers: config.mcp_servers.clone(),
            model_role: config.model_role,
            model_selection: config.model_selection.clone(),
            permission_mode: Some(permission_mode),
            extra_permission_rules: Vec::new(),
            live_permission_rules: Some(live_permission_rules),
            live_permission_mode: Some(live_permission_mode),
            cancel: Some(current_turn_cancel.clone()),
            session_id: config.session_id.clone(),
            agent_id: config.identity.agent_id.clone(),
        };

        // Run query — wrapped in the teammate's task-local identity context
        // so identity-aware tools resolve THIS worker via tier-1 of the
        // 3-tier resolver. `SendMessage`'s structured `shutdown_response`
        // (and plan-approval) call `respond_to_shutdown`, which reads
        // `get_agent_name()/get_team_name()/get_agent_id()`; without the
        // scope those return `None` for an in-process teammate (no env vars,
        // no dynamic context) and the approval fails closed. Unsafe tools
        // run serially inline on THIS task (`commit_flush`), so the
        // task-local reaches their execution; safe (read-only) tools spawn
        // on a JoinSet and never need identity.
        let query_result_result = {
            let prompt_for_query = current_prompt.clone();
            let mut control_poll_interval =
                tokio::time::interval(Duration::from_millis(POLL_INTERVAL_MS));
            let teammate_context = crate::identity::TeammateContextData {
                agent_id: config.identity.agent_id.clone(),
                agent_name: config.identity.agent_name.clone(),
                team_name: config.identity.team_name.clone(),
                color: config.identity.color.map(|c| c.as_str().to_string()),
                plan_mode_required: config.identity.plan_mode_required,
                // Not consumed during the turn; the resume/transcript path
                // that reads it was removed. Empty keeps tier-1 self-consistent.
                parent_session_id: String::new(),
                // Share THIS teammate's cancel flag so an approved
                // `shutdown_response` (via `signal_self_stop`) breaks the loop
                // on the next `config.cancelled` check below. A rejection
                // never sets it, so the teammate keeps working.
                self_stop_signal: Some(config.cancelled.clone()),
            };
            let query_future = crate::identity::run_with_teammate_context(
                teammate_context,
                engine.run_query(&prompt_for_query, query_config),
            );
            tokio::pin!(query_future);
            loop {
                tokio::select! {
                    result = &mut query_future => break result,
                    _ = control_poll_interval.tick() => {
                        drain_control_messages(&config.identity, &control_state).await;
                    }
                }
            }
        };
        let query_result = match query_result_result {
            Ok(result) => result,
            Err(e) => {
                set_teammate_current_work_cancel(&config, None).await;
                let error_msg = format!("{e}");
                update_teammate_task(
                    &config,
                    TeammateTaskUpdate {
                        error: coco_types::FieldUpdate::Set(error_msg.clone()),
                        ..TeammateTaskUpdate::default()
                    },
                )
                .await;
                run_error = Some(error_msg);
                break;
            }
        };
        set_teammate_current_work_cancel(&config, None).await;

        // Accumulate results
        total_turns += query_result.turns;
        total_input_tokens += query_result.input_tokens;
        total_output_tokens += query_result.output_tokens;
        total_tool_use_count = total_tool_use_count.saturating_add(query_result.tool_use_count);
        // Extract tool names from this query's assistant messages and
        // push them into the activity ring buffer. Driven post-query
        // rather than per-stream-event because the teammate runner
        // doesn't expose a `ToolUseStarted` callback.
        const RECENT_ACTIVITIES_CAP: usize = 5;
        for msg in &query_result.messages {
            let coco_types::Message::Assistant(assistant) = msg.as_ref() else {
                continue;
            };
            // `LlmMessage` is an enum whose Assistant variant carries
            // the content parts vector. Every `Message::Assistant`
            // should hold an `LlmMessage::Assistant`, but match
            // defensively to avoid panicking on shape drift.
            let coco_types::LlmMessage::Assistant { content, .. } = &assistant.message else {
                continue;
            };
            for part in content {
                if let coco_types::AssistantContent::ToolCall(call) = part {
                    last_tool_name = Some(call.tool_name.clone());
                    if recent_activities.len() >= RECENT_ACTIVITIES_CAP {
                        recent_activities.pop_front();
                    }
                    // The committed ToolCall carries its full input — flatten it
                    // to the salient argument so the activity row reads
                    // `Bash(cargo build)` instead of a bare `Bash`.
                    let summary =
                        coco_types::tool_summary::tool_input_summary(&call.tool_name, &call.input);
                    recent_activities.push_back(coco_types::TaskActivity {
                        tool_name: call.tool_name.clone(),
                        summary: (!summary.is_empty()).then_some(summary),
                    });
                }
            }
        }
        all_messages.extend(query_result.messages.iter().cloned());

        // Push the assistant message into the teammate's UI mirror and
        // update progress counters via the unified registry path.
        if let Some(text) = &query_result.response_text {
            update_teammate_task(
                &config,
                TeammateTaskUpdate {
                    append_message: Some(coco_types::TeammateTaskMessage {
                        role: coco_types::MessageRole::Assistant,
                        content: text.clone(),
                        tool_name: None,
                    }),
                    ..TeammateTaskUpdate::default()
                },
            )
            .await;
        }
        update_teammate_task(
            &config,
            TeammateTaskUpdate {
                spinner_verb: coco_types::FieldUpdate::Clear,
                past_tense_verb: coco_types::FieldUpdate::Set("Completed".to_string()),
                ..TeammateTaskUpdate::default()
            },
        )
        .await;
        push_teammate_progress(
            &config,
            total_input_tokens,
            total_output_tokens,
            total_tool_use_count,
            total_turns,
            last_tool_name.clone(),
            recent_activities.iter().cloned().collect(),
        )
        .await;

        // Check cancellation after query
        if query_result.cancelled && !config.cancelled.load(Ordering::Relaxed) {
            last_turn_interrupted = true;
            update_teammate_task(
                &config,
                TeammateTaskUpdate {
                    append_message: Some(coco_types::TeammateTaskMessage {
                        role: coco_types::MessageRole::Assistant,
                        content: "Interrupted by user.".to_string(),
                        tool_name: None,
                    }),
                    ..TeammateTaskUpdate::default()
                },
            )
            .await;
        }

        if config.cancelled.load(Ordering::Relaxed) {
            break;
        }

        if query_result.cancelled {
            current_turn_cancel.cancel();
        }

        // Compaction check — runs at tail-of-turn, before idle
        // notification + wait_for_next_prompt, so the worker doesn't
        // sit idle holding a stale, oversized history.
        let total_tokens = total_input_tokens + total_output_tokens;
        if total_tokens > config.auto_compact_threshold && !all_messages.is_empty() {
            match engine
                .compact_messages(
                    all_messages.clone(),
                    total_tokens,
                    &config.session_id,
                    &config.identity.agent_id,
                )
                .await
            {
                Ok(compacted) if compacted.len() < all_messages.len() => {
                    all_messages = compacted;
                }
                Ok(_) | Err(_) => {
                    // Engine declined / errored — apply the
                    // sliding-window safety valve so we don't grow
                    // unboundedly. 20 messages keeps recent context
                    // while bounding tokens.
                    let keep = all_messages.len().min(20);
                    let start = all_messages.len() - keep;
                    all_messages = all_messages[start..].to_vec();
                }
            }
        }

        // Plan-approval gate — runs between the plan-write turn and the
        // first implementation turn. The model just produced its plan
        // (now in `query_result.response_text`); send it to the leader
        // and block until a matching `PlanApprovalResponse` arrives.
        if plan_approval_pending {
            let plan_content = query_result.response_text.clone().unwrap_or_default();
            let request_id = uuid::Uuid::new_v4().to_string();
            let plan_envelope = mailbox::create_plan_approval_request_message(
                &config.identity.agent_name,
                &request_id,
                "",
                &plan_content,
            );
            let envelope_message = mailbox::TeammateMessage {
                from: config.identity.agent_name.clone(),
                text: plan_envelope,
                timestamp: chrono::Utc::now().to_rfc3339(),
                read: false,
                color: config
                    .identity
                    .color
                    .as_ref()
                    .map(|c| c.as_str().to_string()),
                summary: Some("plan approval request".to_string()),
            };
            let _ = mailbox::write_to_mailbox(
                TEAM_LEAD_NAME,
                envelope_message,
                &config.identity.team_name,
            );

            match crate::runner_loop_wait::wait_for_plan_approval(
                &config.identity,
                &config.cancelled,
                &request_id,
            )
            .await
            {
                None => break,
                Some((true, _)) => {
                    plan_approval_pending = false;
                }
                Some((false, feedback)) => {
                    // Plan rejected — re-prompt the model with the
                    // leader's feedback. The next turn writes a revised
                    // plan, which loops through this gate again.
                    let feedback_text = feedback
                        .unwrap_or_else(|| "Plan rejected by leader. Please revise.".to_string());
                    current_prompt = teammate::format_as_teammate_message(
                        TEAM_LEAD_NAME,
                        &feedback_text,
                        None,
                        Some("plan rejected"),
                    );
                    was_idle = false;
                    continue;
                }
            }
        }

        // Transition to idle
        if !was_idle {
            // TeammateIdle hook: fires the idle event, then checks for a
            // blocking result. A blocking hook prevents idle —
            // the teammate continues working with the hook's feedback
            // injected as the next prompt.
            if let (Some(registry), Some(ctx)) =
                (config.hooks.as_ref(), config.orchestration_ctx.as_ref())
                && !ctx.disable_all_hooks
            {
                match coco_hooks::orchestration::execute_teammate_idle(
                    registry,
                    ctx,
                    &config.identity.agent_name,
                    &config.identity.team_name,
                )
                .await
                {
                    Ok(agg) => {
                        if let Some(err) = agg.blocking_error.as_ref() {
                            tracing::info!(
                                teammate = %config.identity.agent_name,
                                "TeammateIdle hook blocked idle transition; continuing work"
                            );
                            current_prompt = teammate::format_as_teammate_message(
                                TEAM_LEAD_NAME,
                                &format!("TeammateIdle hook feedback:\n{}", err.blocking_error),
                                None,
                                Some("idle prevented"),
                            );
                            was_idle = false;
                            continue;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "TeammateIdle hook failed; proceeding with idle");
                    }
                }
            }

            // Send idle notification to leader
            let idle_text = mailbox::create_idle_notification(
                &config.identity.agent_name,
                Some(if last_turn_interrupted {
                    "interrupted"
                } else {
                    "available"
                }),
                None,
            );
            let message = mailbox::TeammateMessage {
                from: config.identity.agent_name.clone(),
                text: idle_text,
                timestamp: chrono::Utc::now().to_rfc3339(),
                read: false,
                color: config
                    .identity
                    .color
                    .as_ref()
                    .map(|c| c.as_str().to_string()),
                summary: Some("idle".to_string()),
            };
            let _ = mailbox::write_to_mailbox(TEAM_LEAD_NAME, message, &config.identity.team_name);
        }

        update_teammate_task(
            &config,
            TeammateTaskUpdate {
                is_idle: coco_types::FieldUpdate::Set(true),
                ..TeammateTaskUpdate::default()
            },
        )
        .await;
        // Mark as idle to suppress duplicate notifications on next iteration
        was_idle = true;

        // Read was_idle to prevent "value never read" warning.
        // The flag controls idle notification sending above.
        let _ = was_idle;

        // Wait for next prompt or shutdown
        let wait_result = wait_for_next_prompt_or_shutdown(
            &config.identity,
            &config.cancelled,
            config.task_list.as_ref(),
            &control_state,
        )
        .await;

        match wait_result {
            WaitResult::Aborted => break,

            WaitResult::ShutdownRequest { original_text } => {
                let wrapped = teammate::format_as_teammate_message(
                    TEAM_LEAD_NAME,
                    &original_text,
                    None,
                    Some("shutdown request"),
                );
                current_prompt = wrapped;
                was_idle = false;
                // No auto-exit flag: the model decides via its
                // `shutdown_response` (approve ⇒ `signal_self_stop` flips
                // `config.cancelled`; reject ⇒ keep working).

                update_teammate_task(
                    &config,
                    TeammateTaskUpdate {
                        is_idle: coco_types::FieldUpdate::Set(false),
                        shutdown_requested: coco_types::FieldUpdate::Set(true),
                        append_message: Some(coco_types::TeammateTaskMessage {
                            role: coco_types::MessageRole::User,
                            content: "Shutdown requested".to_string(),
                            tool_name: None,
                        }),
                        ..TeammateTaskUpdate::default()
                    },
                )
                .await;
            }

            WaitResult::NewMessage {
                message,
                from,
                color,
                summary,
            } => {
                let wrapped = if from == "user" {
                    message
                } else {
                    teammate::format_as_teammate_message(
                        &from,
                        &message,
                        color.as_deref(),
                        summary.as_deref(),
                    )
                };
                current_prompt = wrapped;
                was_idle = false;

                update_teammate_task(
                    &config,
                    TeammateTaskUpdate {
                        is_idle: coco_types::FieldUpdate::Set(false),
                        append_message: Some(coco_types::TeammateTaskMessage {
                            role: coco_types::MessageRole::User,
                            content: format!("Message from {from}"),
                            tool_name: None,
                        }),
                        ..TeammateTaskUpdate::default()
                    },
                )
                .await;
            }
        }

        // Compaction now runs post-turn at the top of the loop body —
        // see the moved block right after the cancellation check above.
    }

    // Unified cleanup path — runs whether the loop exited normally,
    // via cancellation, via shutdown-reply completion, or via query
    // error. Earlier code split the error case out as an early return
    // and skipped the coordinator-mode notification.
    let stop_reason = run_error.as_ref().map(|msg| format!("failed: {msg}"));
    teammate::on_teammate_stop(
        &config.identity.agent_name,
        &config.identity.team_name,
        config.identity.color.as_ref().map(|c| c.as_str()),
        stop_reason.as_deref(),
    );
    if let Some(store) = &config.roster_store
        && let Err(e) = store
            .set_member_active(SetMemberActiveRequest {
                team_name: config.identity.team_name.clone(),
                member_name: config.identity.agent_name.clone(),
                is_active: false,
            })
            .await
    {
        tracing::warn!(error = %e, "failed to mark teammate inactive");
    }

    // Coordinator-mode notification: when the leader is operating as a
    // coordinator (`COCO_COORDINATOR_MODE=1` + `Feature::AgentTeams`),
    // push a `<task-notification>` XML envelope to the leader's mailbox
    // so the model receives the structured worker-termination signal.
    // Status reflects the actual outcome — `Completed` on clean exit,
    // `Failed` on query error.
    if let Some(features) = config.features.as_deref()
        && coco_subagent::is_coordinator_mode(features)
    {
        let agent_id = format!(
            "{}@{}",
            config.identity.agent_name, config.identity.team_name
        );
        let (status, summary, result) = match run_error.as_deref() {
            Some(err) => (
                coco_subagent::TaskNotificationStatus::Failed,
                format!("Agent \"{}\" failed", config.identity.agent_name),
                Some(err),
            ),
            None => (
                coco_subagent::TaskNotificationStatus::Completed,
                format!("Agent \"{}\" completed", config.identity.agent_name),
                None,
            ),
        };
        let xml = coco_subagent::render_task_notification(&coco_subagent::TaskNotification {
            task_id: &agent_id,
            status,
            summary: &summary,
            result,
            usage: Some(coco_subagent::TaskNotificationUsage {
                total_tokens: total_input_tokens + total_output_tokens,
                tool_uses: 0,
                duration_ms: 0,
            }),
        });
        let _ = crate::runner_loop_notify::send_message_to_leader(
            &config.identity.agent_name,
            &xml,
            config.identity.color.as_ref().map(|c| c.as_str()),
            &config.identity.team_name,
        );
    }

    update_teammate_task(
        &config,
        TeammateTaskUpdate {
            is_idle: coco_types::FieldUpdate::Set(true),
            spinner_verb: coco_types::FieldUpdate::Clear,
            ..TeammateTaskUpdate::default()
        },
    )
    .await;

    let success = run_error.is_none();
    InProcessRunnerResult {
        success,
        error: run_error,
        output: None,
        turns: total_turns,
        total_input_tokens,
        total_output_tokens,
    }
}

async fn update_teammate_task(config: &InProcessRunnerConfig, update: TeammateTaskUpdate) {
    if let Some(registry) = &config.task_registry {
        registry
            .update_teammate_task(&config.identity.agent_id, update)
            .await;
    }
}

async fn push_teammate_progress(
    config: &InProcessRunnerConfig,
    input_tokens: i64,
    output_tokens: i64,
    tool_use_count: i32,
    turn_count: i32,
    last_tool_name: Option<String>,
    recent_activities: Vec<coco_types::TaskActivity>,
) {
    let Some(registry) = &config.task_registry else {
        return;
    };
    // Look up the task_id for this teammate so we can call set_progress.
    let Some(state) = registry
        .teammate_task_state(&config.identity.agent_id)
        .await
    else {
        return;
    };
    let total_tokens = input_tokens.saturating_add(output_tokens);
    registry
        .set_progress(
            &state.id,
            coco_types::TaskProgress {
                input_tokens,
                output_tokens,
                total_tokens,
                cache_read_tokens: 0,
                cost_micro_usd: 0,
                agent_type: None,
                tool_use_count,
                turn_count,
                last_tool_name,
                recent_activities,
                summary: None,
            },
        )
        .await;
}

async fn set_teammate_current_work_cancel(
    config: &InProcessRunnerConfig,
    cancel: Option<CancellationToken>,
) {
    if let Some(registry) = &config.task_registry {
        let _ = registry
            .set_teammate_current_work_cancel(&config.identity.agent_id, cancel)
            .await;
    }
}

// ── Wait For Next Prompt ──

/// Poll interval for mailbox scanning (ms).
const POLL_INTERVAL_MS: u64 = 500;

/// Wait for the next prompt, shutdown request, or abort.
///
/// Priority order:
/// 1. Abort signal check.
/// 2. Mailbox messages:
///    a. Shutdown requests (highest mailbox priority).
///    b. Team-lead messages (second — represents user intent).
///    c. Peer messages (FIFO, third).
/// 3. Unclaimed tasks from task list (lowest).
///
/// **Implementation gap, intentional**: additionally draining
/// `pendingUserMessages` — messages the user typed into the teammate's
/// transcript-view UI — is not yet implemented. coco-rs has no
/// transcript-view UI yet, so the queue's only producer doesn't exist.
/// When the TUI lands, the right port is an
/// `mpsc::UnboundedReceiver<String>` registered per-`agent_id` on
/// `InProcessAgentRunner`, drained at the top of this loop above the
/// abort check.
pub(crate) async fn wait_for_next_prompt_or_shutdown(
    identity: &TeammateIdentity,
    cancelled: &AtomicBool,
    task_list: Option<&TaskListHandleRef>,
    control_state: &RwLock<TeammateControlState>,
) -> WaitResult {
    let mut poll_count = 0u64;

    loop {
        // Sleep (skip first iteration for responsiveness)
        if poll_count > 0 {
            tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
        }
        poll_count += 1;

        // Priority 1: Abort check
        if cancelled.load(Ordering::Relaxed) {
            return WaitResult::Aborted;
        }

        // Priority 2: Mailbox scanning. Read once (pre-drain snapshot) and
        // hand the same snapshot to the prompt scan — matches the historical
        // single pre-`drain_control_messages` read so behavior is unchanged.
        let messages =
            mailbox::read_mailbox(&identity.agent_name, &identity.team_name).unwrap_or_default();

        // 2a: Control messages that update local teammate state and
        // should not become model prompts. Drains its own read + marks read.
        drain_control_messages(identity, control_state).await;

        // 2b–3: shutdown / team-lead / peer / unclaimed-task priority scan.
        if let Some(result) = scan_next_prompt(identity, task_list, &messages).await {
            return result;
        }
    }
}

/// One non-blocking pass of the teammate mailbox + task list, in the same
/// priority order as the in-process [`wait_for_next_prompt_or_shutdown`]
/// loop body: shutdown-request > team-lead message > peer FIFO >
/// unclaimed task. Marks the **returned** message read; never returns
/// [`WaitResult::Aborted`] (abort is the caller's concern) and does NOT
/// drain control messages.
///
/// Shared by the in-process runner (which sleeps + loops around it under
/// its own abort + control-drain) and the cross-process teammate inbox
/// pump (`app/cli::teammate_inbox_pump`, gap 1) which ticks it on an
/// interval and injects the framed result as a TUI turn.
///
/// `messages` is a caller-provided snapshot so the in-process loop can
/// pass the same pre-`drain_control_messages` read it already performs.
///
/// Prompt-bearing means a **plain-text** mailbox entry (or an unclaimed
/// task). Every structured protocol message
/// ([`mailbox::is_structured_protocol_message`]) is skipped here so it can
/// only ever reach its dedicated consumer — the shutdown arm below,
/// `drain_control_messages` (ModeSet / TeamPermissionUpdate), the permission
/// bridge (PermissionResponse), or `wait_for_plan_approval`
/// (PlanApprovalResponse). This prevents a stray response/notification that
/// landed in the teammate's own inbox from being mis-injected as a model
/// prompt — a real hazard for the cross-process pump, which (unlike the
/// in-process runner) has no in-turn consumer draining those before the scan.
pub async fn scan_next_prompt(
    identity: &TeammateIdentity,
    task_list: Option<&TaskListHandleRef>,
    messages: &[mailbox::TeammateMessage],
) -> Option<WaitResult> {
    // 2b–2d: highest-priority prompt-bearing mailbox entry. Pure selection,
    // then mark the chosen message read (re-reads the file by index; the
    // append-only inbox keeps the snapshot index aligned with disk).
    if let Some((index, result)) = select_mailbox_prompt(messages) {
        let _ = mailbox::mark_message_as_read_by_index(
            &identity.agent_name,
            &identity.team_name,
            index,
        );
        return Some(result);
    }

    // Priority 3: Unclaimed tasks (lowest)
    if let Some(task_list) = task_list
        && let Some(task) = claim_first_available_task(task_list, &identity.agent_name).await
    {
        return Some(WaitResult::NewMessage {
            message: crate::runner_loop_notify::format_task_as_prompt(
                &task.id,
                task.active_form.as_deref().unwrap_or(&task.subject),
                &task.description,
            ),
            from: TEAM_LEAD_NAME.to_string(),
            color: None,
            summary: Some("task list assignment".to_string()),
        });
    }

    None
}

/// Pure mailbox-arm selection — no I/O, no task list. Returns the index of
/// the highest-priority prompt-bearing message and the [`WaitResult`] to
/// emit, in the order shutdown-request > team-lead text > peer FIFO text.
///
/// A message is prompt-bearing only when it is **plain text** (not a
/// structured protocol message). The single exception is a `ShutdownRequest`,
/// which has its own top-priority arm; every other structured message
/// (PermissionResponse / PlanApprovalResponse / ModeSet / … ) is skipped so
/// it can only reach its dedicated consumer, never the model as a prompt.
fn select_mailbox_prompt(messages: &[mailbox::TeammateMessage]) -> Option<(usize, WaitResult)> {
    // 2b: Shutdown requests (highest prompt-bearing mailbox priority)
    for (i, msg) in messages.iter().enumerate() {
        if msg.read {
            continue;
        }
        if mailbox::is_structured_protocol_message(&msg.text)
            && let Some(protocol) = mailbox::parse_protocol_message(&msg.text)
            && matches!(protocol, mailbox::ProtocolMessage::ShutdownRequest { .. })
        {
            return Some((
                i,
                WaitResult::ShutdownRequest {
                    original_text: msg.text.clone(),
                },
            ));
        }
    }

    // 2c: Team-lead messages (second priority)
    if let Some((i, msg)) = messages.iter().enumerate().find(|(_, m)| {
        !m.read && m.from == TEAM_LEAD_NAME && !mailbox::is_structured_protocol_message(&m.text)
    }) {
        return Some((i, new_message_from(msg)));
    }

    // 2d: Any unread plain-text message (peer FIFO, third priority)
    if let Some((i, msg)) = messages
        .iter()
        .enumerate()
        .find(|(_, m)| !m.read && !mailbox::is_structured_protocol_message(&m.text))
    {
        return Some((i, new_message_from(msg)));
    }

    None
}

fn new_message_from(msg: &mailbox::TeammateMessage) -> WaitResult {
    WaitResult::NewMessage {
        message: msg.text.clone(),
        from: msg.from.clone(),
        color: msg.color.clone(),
        summary: msg.summary.clone(),
    }
}

async fn drain_control_messages(
    identity: &TeammateIdentity,
    control_state: &RwLock<TeammateControlState>,
) {
    let messages =
        mailbox::read_mailbox(&identity.agent_name, &identity.team_name).unwrap_or_default();
    for (i, msg) in messages.iter().enumerate() {
        if msg.read || msg.from != TEAM_LEAD_NAME {
            continue;
        }
        if !mailbox::is_structured_protocol_message(&msg.text) {
            continue;
        }
        let Some(protocol) = mailbox::parse_protocol_message(&msg.text) else {
            continue;
        };
        match protocol {
            mailbox::ProtocolMessage::ModeSetRequest { mode, .. } => {
                let mode_store = {
                    let state = control_state.read().await;
                    state.permission_mode.clone()
                };
                *mode_store.write().await = mode;
                let _ = mailbox::mark_message_as_read_by_index(
                    &identity.agent_name,
                    &identity.team_name,
                    i,
                );
            }
            mailbox::ProtocolMessage::TeamPermissionUpdate {
                permission_update, ..
            } => {
                let rules = permission_update.into_permission_rules();
                if !rules.is_empty() {
                    let rule_store = {
                        let state = control_state.read().await;
                        state.team_permission_rules.clone()
                    };
                    rule_store.write().await.extend(rules);
                }
                let _ = mailbox::mark_message_as_read_by_index(
                    &identity.agent_name,
                    &identity.team_name,
                    i,
                );
            }
            _ => {}
        }
    }
}

// ── Task Management Helpers ──

async fn claim_first_available_task(
    task_list: &TaskListHandleRef,
    claimant: &str,
) -> Option<coco_types::TaskRecord> {
    let tasks = match task_list.list_tasks().await {
        Ok(tasks) => tasks,
        Err(e) => {
            tracing::warn!(error = %e, "failed to list team tasks");
            return None;
        }
    };

    let unresolved_task_ids = tasks
        .iter()
        .filter(|task| task.status != coco_types::TaskListStatus::Completed)
        .map(|task| task.id.clone())
        .collect::<std::collections::HashSet<_>>();

    for task in tasks {
        if task.status != coco_types::TaskListStatus::Pending
            || task.owner.is_some()
            || task
                .blocked_by
                .iter()
                .any(|id| unresolved_task_ids.contains(id))
        {
            continue;
        }

        let claimed = match task_list.claim_task(&task.id, claimant, true).await {
            Ok(coco_types::TaskClaimOutcome::Success(task)) => task,
            Ok(_) => continue,
            Err(e) => {
                tracing::warn!(task_id = %task.id, error = %e, "failed to claim team task");
                continue;
            }
        };

        match task_list
            .update_task(
                &claimed.id,
                coco_types::TaskRecordUpdate {
                    status: Some(coco_types::TaskListStatus::InProgress),
                    ..Default::default()
                },
            )
            .await
        {
            Ok(Some(task)) => return Some(task),
            Ok(None) => return Some(claimed),
            Err(e) => {
                tracing::warn!(task_id = %claimed.id, error = %e, "failed to mark team task in progress");
                return Some(claimed);
            }
        }
    }

    None
}

/// Read a team's `team_allowed_paths` and convert them into `Allow`
/// permission rules. Used to seed a teammate's live permission rules so it
/// inherits the team's allowed write paths without prompting — by the
/// in-process runner directly and by the cross-process teammate boot path
/// (gap 8 `team_allowed_paths` seeding).
pub fn load_team_allowed_path_rules(team_name: &str) -> Vec<coco_types::PermissionRule> {
    let Ok(Some(team_file)) = crate::team_file::read_team_file(team_name) else {
        return Vec::new();
    };
    team_file
        .team_allowed_paths
        .iter()
        .map(|allowed| {
            permission_rule(
                &allowed.tool_name,
                Some(team_allowed_path_rule_content(&allowed.path)),
                coco_types::PermissionBehavior::Allow,
            )
        })
        .collect()
}

fn team_allowed_path_rule_content(path: &str) -> String {
    if path.starts_with('/') {
        format!("/{path}/**")
    } else {
        format!("{path}/**")
    }
}

fn permission_rule(
    tool_name: &str,
    rule_content: Option<String>,
    behavior: coco_types::PermissionBehavior,
) -> coco_types::PermissionRule {
    coco_types::PermissionRule {
        source: coco_types::PermissionRuleSource::Session,
        behavior,
        value: coco_types::PermissionRuleValue {
            tool_pattern: tool_name.to_string(),
            rule_content,
        },
    }
}

/// Fire-and-forget entry point for starting a teammate.
///
/// Spawns [`run_in_process_teammate`] on the Tokio runtime and returns
/// the join handle.
pub fn start_in_process_teammate(
    config: InProcessRunnerConfig,
    engine: std::sync::Arc<dyn AgentExecutionEngine>,
) -> tokio::task::JoinHandle<InProcessRunnerResult> {
    tokio::spawn(async move { run_in_process_teammate(config, engine.as_ref()).await })
}

#[cfg(test)]
#[path = "runner_loop.test.rs"]
mod tests;
