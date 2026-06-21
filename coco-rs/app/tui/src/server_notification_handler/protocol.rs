//! Protocol-layer handler.
//!
//! Handles all [`ServerNotification`] variants. The TUI matches
//! exhaustively — adding a new variant in `coco-types` fails compilation
//! here until a TUI behavior is chosen (even if that behavior is an
//! explicit `false` no-op with a comment).
//!
//! Subagent / teammate lifecycle is **not** a separate event family on the
//! wire — it rides on [`ServerNotification::TaskStarted`] /
//! [`ServerNotification::TaskProgress`] / [`ServerNotification::TaskCompleted`]
//! with [`TaskStartedParams::task_type`] discriminating (`bg_agent`,
//! `in_process_teammate`, `shell`, `dream`, …). There are no `subagent/*`
//! SDK events on the wire. See the `TaskStarted` arm below for the
//! `SubagentInstance` projection.
//!
//! Item lifecycle (`ItemStarted`, `ItemUpdated`, `ItemCompleted`,
//! `AgentMessageDelta`, `ReasoningDelta`) are intentionally no-ops: they're
//! produced by `StreamAccumulator` for SDK consumers and never reach the
//! TUI channel in the current architecture. See `event-system-design.md`
//! §12 for the consumer routing matrix.

use coco_types::ServerNotification;
use coco_types::TaskStartedParams;
use coco_types::task_type_wire;

use crate::i18n::t;
use crate::state::AppState;
use crate::state::ModalState;
use crate::state::PanePromptState;
use crate::state::session::HookEntry;
use crate::state::session::HookEntryStatus;
use crate::state::session::McpServerStatus;
use crate::state::session::RateLimitInfo;
use crate::state::session::SubagentInstance;
use crate::state::session::SubagentKind;
use crate::state::session::SubagentStatus;
use crate::state::session::TaskEntry;
use crate::state::session::TaskEntryKind;
use crate::state::session::TaskEntryStatus;
use crate::state::ui::Toast;

mod turn;
use turn::clear_session_boundary_state;
use turn::on_turn_ended;
#[cfg(test)]
use turn::on_turn_interrupted_outcome;

pub(super) fn handle(
    state: &mut AppState,
    notif: ServerNotification,
    command_tx: &tokio::sync::mpsc::Sender<crate::command::UserCommand>,
) -> bool {
    match notif {
        // === Session lifecycle ===
        ServerNotification::SessionStarted(p) => {
            state.session.session_id = Some(p.session_id);
            state.session.output_style = p.output_style;
            state.session.session_usage = None;
            state.session.token_usage = crate::state::session::TokenUsage::default();
            // Initialise thinking_effort from the model's registered
            // default so the status bar reflects the starting state
            // before any Ctrl+T cycle. Falls back to Auto when the
            // model has no opinion (the resting state).
            state.session.thinking_effort = coco_config::builtin_models_partial()
                .get(&p.model)
                .and_then(|info| info.default_thinking_level)
                .unwrap_or(coco_types::ReasoningEffort::Auto);
            state.session.model = p.model;
            if !p.provider.is_empty() {
                state.session.provider = p.provider;
            }
            // Resolve the current git branch before storing cwd so a
            // failure (cwd outside a repo, detached HEAD) leaves
            // `git_branch = None` rather than a stale value. Stdout
            // shells out to `git rev-parse`; this fires once per session
            // start so the cost is negligible.
            let cwd_path = std::path::PathBuf::from(&p.cwd);
            state.session.git_branch = coco_git::get_current_branch(&cwd_path).ok().flatten();
            state.session.working_dir = Some(p.cwd);
            // A new session invalidates any cached agent markdown — otherwise
            // /copy could surface text from a previous conversation.
            state.session.last_agent_markdown = None;
            // LSP active flag drives the status-bar badge. Stays sticky
            // for the lifetime of the session — a per-server refresh
            // would need a separate `LspPrewarmComplete` event (variant
            // is defined, emission TBD when settings reload triggers
            // re-prewarm).
            state.session.lsp_active = p.lsp_active;
            true
        }
        ServerNotification::SessionResult(p) => {
            state.session.estimated_cost_cents = (p.total_cost_usd * 100.0) as i32;
            true
        }
        ServerNotification::SessionUsageUpdated(snapshot) => {
            let snapshot = *snapshot;
            state.session.token_usage = token_usage_from_session_snapshot(&snapshot);
            state.session.session_usage = Some(snapshot);
            true
        }
        ServerNotification::SessionEnded(p) => {
            let _ = p.reason;
            state.quit();
            true
        }

        // === Turn lifecycle ===
        ServerNotification::TurnStarted(p) => {
            // `TurnStarted` is per logical user-prompt cycle. Increment
            // the local cumulative count rather than reading it off the
            // wire — the wire field used to be hardcoded to `1` at every
            // runner, so reading it always overwrote the footer back to
            // 1. The runner doesn't know cumulative session-level turn
            // count anyway; the TUI is the only consumer that needs the
            // running total.
            state.session.turn_count = state.session.turn_count.saturating_add(1);
            let _ = &p.turn_id;
            state.session.set_busy(true);
            state.session.stream_stall = false;
            // Reset pause accumulators + sample a fresh spinner verb +
            // anchor the turn start. The loading time ref is re-anchored at
            // turn start; the spinner verb is sampled once per turn.
            state.ui.ephemeral.start_turn(
                coco_tui_ui::widgets::spinner_verbs::pick_verb_random(),
                state.clock.now(),
            );
            state.ui.streaming = Some(crate::state::ui::StreamingState::new());
            true
        }
        // Unified terminal event: `TurnEnded` discriminates by
        // `outcome.kind`. The four (now five) old separate arms collapse
        // into one match. Each outcome routes to a dedicated handler so
        // bodies stay focused.
        // [F11 anchor] None of these dispatch follow-up UserCommands
        // except `Interrupted` (auto-restore path).
        ServerNotification::TurnEnded(p) => on_turn_ended(state, p, command_tx),

        // === Item lifecycle (SDK-path only; TUI uses Stream layer) ===
        // The TUI gets real-time display from AgentStreamEvent (TextDelta,
        // ToolUseQueued, etc.) in handle_stream(). The Item* protocol events
        // are produced by StreamAccumulator for SDK consumers and are
        // intentionally no-ops in the TUI.
        ServerNotification::ItemStarted { .. }
        | ServerNotification::ItemUpdated { .. }
        | ServerNotification::ItemCompleted { .. } => false,

        // === Content deltas (SDK path — TUI uses Stream layer) ===
        ServerNotification::AgentMessageDelta(_) | ServerNotification::ReasoningDelta(_) => false,

        // === MCP ===
        ServerNotification::McpStartupStatus(p) => {
            let connected = matches!(p.status, coco_types::McpConnectionStatus::Connected);
            if let Some(server) = state
                .session
                .mcp_servers
                .iter_mut()
                .find(|s| s.name == p.server)
            {
                server.connected = connected;
            } else {
                state.session.mcp_servers.push(McpServerStatus {
                    name: p.server,
                    connected,
                    tool_count: 0,
                });
            }
            true
        }
        ServerNotification::McpStartupComplete(p) => {
            if !p.failed.is_empty() {
                state.ui.add_toast(Toast::warning(
                    t!("toast.mcp_failed_to_start", count = p.failed.len()).to_string(),
                ));
            }
            true
        }

        // === LSP ===
        ServerNotification::LspPrewarmComplete(p) => {
            // Mid-session prewarm completed (e.g. settings reload).
            // Flip the badge based on whether the new prewarm spawned
            // anything. SessionStarted carries the bootstrap value;
            // this state handles subsequent state changes.
            state.session.lsp_active = !p.started.is_empty();
            true
        }

        // === Context ===
        ServerNotification::ContextCompacted(p) => {
            state.session.is_compacting = false;
            state.session.compaction_started_at = None;
            state.session.compaction_phase = None;
            // Suppress the next ContextUsageWarning emission — the
            // freshly-compacted token count won't be reflected in the
            // banner until the next API response arrives.
            state.session.compact_warning_suppressed = true;
            state.ui.add_toast(Toast::info(
                t!("toast.compacted_short", count = p.removed_messages).to_string(),
            ));
            true
        }
        ServerNotification::ContextUsageWarning(p) => {
            // The next API response will deliver an accurate token count;
            // until then, drop the warning so we don't show a stale value.
            if !state.session.compact_warning_suppressed && p.percent_left < 10.0 {
                state.ui.add_toast(Toast::warning(format!(
                    "Context {:.0}% remaining",
                    p.percent_left
                )));
            }
            // Any subsequent warning post-suppression means the API
            // already returned a fresh count — clear the flag.
            state.session.compact_warning_suppressed = false;
            true
        }
        ServerNotification::CompactionStarted => {
            state.session.is_compacting = true;
            state.session.compaction_started_at = Some(std::time::Instant::now());
            state.session.compact_warning_suppressed = false;
            true
        }
        ServerNotification::CompactionPhase(p) => {
            use crate::state::session::CompactionPhaseLabel;
            use coco_types::CompactionHookType;
            use coco_types::CompactionPhase;
            state.session.compaction_phase = match (p.phase, p.hook_type) {
                (CompactionPhase::HooksStart, Some(CompactionHookType::PreCompact)) => {
                    state.session.is_compacting = true;
                    state
                        .session
                        .compaction_started_at
                        .get_or_insert_with(std::time::Instant::now);
                    Some(CompactionPhaseLabel::PreCompactHooks)
                }
                (CompactionPhase::HooksStart, Some(CompactionHookType::PostCompact)) => {
                    state.session.is_compacting = true;
                    state
                        .session
                        .compaction_started_at
                        .get_or_insert_with(std::time::Instant::now);
                    Some(CompactionPhaseLabel::PostCompactHooks)
                }
                (CompactionPhase::HooksStart, Some(CompactionHookType::SessionStart)) => {
                    state.session.is_compacting = true;
                    state
                        .session
                        .compaction_started_at
                        .get_or_insert_with(std::time::Instant::now);
                    Some(CompactionPhaseLabel::SessionStartHooks)
                }
                (CompactionPhase::HooksStart, None) => {
                    state.session.is_compacting = true;
                    state
                        .session
                        .compaction_started_at
                        .get_or_insert_with(std::time::Instant::now);
                    Some(CompactionPhaseLabel::PreCompactHooks)
                }
                (CompactionPhase::Summarizing, _) => {
                    state.session.is_compacting = true;
                    state
                        .session
                        .compaction_started_at
                        .get_or_insert_with(std::time::Instant::now);
                    Some(CompactionPhaseLabel::Summarizing)
                }
                (CompactionPhase::Done, _) => {
                    state.session.is_compacting = false;
                    state.session.compaction_started_at = None;
                    state.session.compact_warning_suppressed = true;
                    None
                }
            };
            true
        }
        ServerNotification::CompactionFailed(p) => {
            state.session.is_compacting = false;
            state.session.compaction_started_at = None;
            state.session.compaction_phase = None;
            // Compaction failures leave the session in a compromised state
            // (context still over budget); escalate past the toast.
            let msg = t!("toast.compaction_failed_short", error = p.error.as_str()).to_string();
            state.ui.add_toast(Toast::error(msg.clone()));
            let body =
                crate::widgets::error_dialog::format_error_body(&msg, Some("compaction"), false);
            state.ui.show_modal(ModalState::Error(body));
            true
        }
        ServerNotification::ContextCleared(_) => {
            state
                .ui
                .add_toast(Toast::info(t!("toast.context_cleared").to_string()));
            true
        }

        // === Task ===
        ServerNotification::TaskStarted(p) => {
            // Spawn projection for subagents and teammates. The teammate
            // roster metadata (`agent_name` / `team_name` / `color` /
            // `backend_kind`) rides on `TaskStarted` because the TUI sits
            // across a process boundary.
            //
            // Subagent (BgAgent) rows leave those `None` — they're
            // discriminated by `task_type == "local_agent"` and identified
            // by `task_id`. See `coco_tasks::task_type_wire_name` for the
            // canonical wire strings.
            match p.task_type.as_deref() {
                Some(s) if s == task_type_wire::LOCAL_AGENT => {
                    ensure_subagent_row(state, SubagentKind::Subagent, &p);
                }
                Some(s) if s == task_type_wire::IN_PROCESS_TEAMMATE => {
                    ensure_subagent_row(state, SubagentKind::Teammate, &p);
                }
                _ => {}
            }
            let kind = match p.task_type.as_deref() {
                Some(s) if s == task_type_wire::LOCAL_BASH => TaskEntryKind::Shell,
                Some(s)
                    if s == task_type_wire::LOCAL_AGENT
                        || s == task_type_wire::IN_PROCESS_TEAMMATE
                        || s == task_type_wire::REMOTE_AGENT =>
                {
                    TaskEntryKind::Agent
                }
                _ => TaskEntryKind::Other,
            };
            let started_at_ms = state.clock.now_ms();
            state.session.active_tasks.push(TaskEntry {
                task_id: p.task_id,
                description: p.description,
                status: TaskEntryStatus::Running,
                kind,
                started_at_ms,
            });
            true
        }
        ServerNotification::TaskCompleted(p) => {
            let entry_status = match p.status {
                coco_types::TaskCompletionStatus::Completed => TaskEntryStatus::Completed,
                coco_types::TaskCompletionStatus::Failed => TaskEntryStatus::Failed,
                coco_types::TaskCompletionStatus::Stopped => TaskEntryStatus::Stopped,
            };
            if let Some(task) = state
                .session
                .active_tasks
                .iter_mut()
                .find(|t| t.task_id == p.task_id)
            {
                task.status = entry_status;
            }
            // Mirror onto the SubagentInstance projection — pair with the
            // BgAgent bridge in `TaskStarted`. `Stopped` (wire mapping of
            // `TaskStatus::Killed`) is a terminal failure from the user's
            // perspective; the orthogonal `is_backgrounded` flag is owned
            // by the optimistic Ctrl+B flip in `update.rs`, never by a
            // terminal status.
            let now = state.clock.now_ms();
            // Captured inside the agent borrow, inserted after it drops —
            // the completed subagent's run summary for the transcript cell.
            let mut completed_summary: Option<(String, crate::state::session::SubagentRunSummary)> =
                None;
            if let Some(agent) = state
                .session
                .subagents
                .iter_mut()
                .find(|a| a.agent_id == p.task_id)
            {
                let prev_status = agent.status;
                agent.status = match p.status {
                    coco_types::TaskCompletionStatus::Completed => SubagentStatus::Completed,
                    coco_types::TaskCompletionStatus::Failed
                    | coco_types::TaskCompletionStatus::Stopped => SubagentStatus::Failed,
                };
                // Freeze the timer at completion so the row stops ticking
                // with its still-running siblings. Stamp once (gate on the
                // Running→terminal edge) so a duplicate TaskCompleted can't
                // move the frozen time.
                if matches!(prev_status, SubagentStatus::Running) {
                    agent.completed_at_ms = Some(now);
                }
                // Authoritative final tokens + cost (TUI panel + the
                // `Subagents · N tok · $X` aggregate line).
                if let Some(usage) = &p.usage {
                    agent.total_tokens = agent.total_tokens.max(usage.total_tokens);
                    agent.input_tokens = agent.input_tokens.max(usage.input_tokens);
                    agent.output_tokens = agent.output_tokens.max(usage.output_tokens);
                    agent.cache_read_tokens = agent.cache_read_tokens.max(usage.cache_read_tokens);
                    if usage.cost_usd > 0.0 {
                        agent.cost_usd = usage.cost_usd;
                    }
                }
                if !p.summary.is_empty() {
                    agent.final_message = Some(preview_summary(&p.summary, 80));
                }
                // Capture the run summary on the Running→terminal edge so it
                // lands on the spawning Agent tool-result cell once the agent
                // drops out of the transient panel.
                if matches!(prev_status, SubagentStatus::Running)
                    && let Some(tuid) = p.tool_use_id.clone()
                {
                    let duration_ms = agent.started_at_ms.map(|s| (now - s).max(0)).unwrap_or(0);
                    completed_summary = Some((
                        tuid,
                        crate::state::session::SubagentRunSummary {
                            agent_type: agent.agent_type.clone(),
                            tool_count: agent.tool_count,
                            duration_ms,
                            input_tokens: agent.input_tokens,
                            output_tokens: agent.output_tokens,
                            cache_read_tokens: agent.cache_read_tokens,
                            cost_usd: agent.cost_usd,
                            succeeded: matches!(agent.status, SubagentStatus::Completed),
                        },
                    ));
                }
                // Toast only when the agent was explicitly backgrounded
                // (Ctrl+B) — foreground completions are already visible
                // in the activity panel. Gate on the Running→terminal
                // transition so a duplicate TaskCompleted doesn't fire
                // the toast twice.
                if matches!(prev_status, SubagentStatus::Running) && agent.is_backgrounded {
                    let label = agent.description.clone();
                    let toast = match agent.status {
                        SubagentStatus::Completed => Toast::info(
                            t!("toast.subagent_background_done", name = label.as_str()).to_string(),
                        ),
                        SubagentStatus::Failed => Toast::warning(
                            t!("toast.subagent_background_failed", name = label.as_str())
                                .to_string(),
                        ),
                        SubagentStatus::Running => unreachable!(),
                    };
                    state.ui.add_toast(toast);
                }
            }
            if let Some((tuid, summary)) = completed_summary {
                state.session.insert_subagent_summary(tuid, summary);
            }
            true
        }
        ServerNotification::TaskProgress(p) => {
            if let Some(task) = state
                .session
                .active_tasks
                .iter_mut()
                .find(|t| t.task_id == p.task_id)
            {
                task.description = p.description.clone();
            }
            // BgAgent task IDs are also subagent IDs (see `TaskStateBase
            // ::identity` in coco-types). Mirror the progress counters
            // onto the matching `SubagentInstance` so the activity
            // panel can render `<last_tool> · N tools · M tok`.
            // Counters are monotonically maxed so an out-of-order
            // snapshot can't roll them backwards.
            if let Some(agent) = state
                .session
                .subagents
                .iter_mut()
                .find(|a| a.agent_id == p.task_id)
            {
                // Coordinator-side rings (`runner_loop.rs`,
                // `agent_handle/spawn.rs`) enforce the cap-5 ring buffer
                // (`MAX_RECENT_ACTIVITIES = 5`). The TUI does not re-cap
                // on receive — it trusts the producer. Copying the slice
                // verbatim avoids the earlier `last_tool_name`-only fallback
                // that dropped intermediate tools whenever multiple calls
                // fired between progress events.
                if !p.recent_activities.is_empty() {
                    agent.recent_activities = p.recent_activities.clone();
                }
                if p.last_tool_name.is_some() {
                    agent.last_tool_name = p.last_tool_name.clone();
                }
                agent.tool_count = agent.tool_count.max(p.usage.tool_uses);
                agent.total_tokens = agent.total_tokens.max(p.usage.total_tokens);
                agent.input_tokens = agent.input_tokens.max(p.usage.input_tokens);
                agent.output_tokens = agent.output_tokens.max(p.usage.output_tokens);
                agent.cache_read_tokens = agent.cache_read_tokens.max(p.usage.cache_read_tokens);
                // Live spend from the child's per-round usage snapshot
                // (`SessionUsageUpdated` → drain → `set_progress`). Monotonic:
                // the engine total only grows, so the final completion payload
                // and this can't disagree downward.
                if p.usage.cost_usd > agent.cost_usd {
                    agent.cost_usd = p.usage.cost_usd;
                }
                // Replace the `local_agent` wire fallback (all `TaskStarted`
                // knows) with the real declared type once it arrives on the
                // first progress.
                if let Some(agent_type) = p.agent_type.as_deref()
                    && !agent_type.is_empty()
                {
                    agent.agent_type = agent_type.to_string();
                }
            }
            true
        }
        ServerNotification::TaskPanelChanged(p) => {
            // Unified snapshot refresh. Before we replace the snapshot, diff
            // the old/new statuses so we can stamp per-task `Completed`
            // timestamps (`RECENT_COMPLETED_TTL_MS = 30_000` priority lift)
            // and detect the "all completed" transition
            // (`HIDE_DELAY_MS = 5_000` panel auto-hide).
            let now = state.clock.now_ms();
            let prev_statuses: std::collections::HashMap<&str, coco_types::TaskListStatus> = state
                .session
                .plan_tasks
                .iter()
                .map(|t| (t.id.as_str(), t.status))
                .collect();
            for new_task in &p.plan_tasks {
                let prev = prev_statuses.get(new_task.id.as_str()).copied();
                let was_completed = matches!(prev, Some(coco_types::TaskListStatus::Completed));
                let now_completed =
                    matches!(new_task.status, coco_types::TaskListStatus::Completed);
                if now_completed && !was_completed {
                    state
                        .ui
                        .ephemeral
                        .task_completion_timestamps
                        .insert(new_task.id.clone(), now);
                } else if !now_completed && was_completed {
                    state
                        .ui
                        .ephemeral
                        .task_completion_timestamps
                        .remove(new_task.id.as_str());
                }
            }
            // Garbage-collect timestamps for tasks that disappeared.
            let live_ids: std::collections::HashSet<&str> =
                p.plan_tasks.iter().map(|t| t.id.as_str()).collect();
            state
                .ui
                .ephemeral
                .task_completion_timestamps
                .retain(|id, _| live_ids.contains(id.as_str()));

            state.session.plan_tasks = p.plan_tasks;
            state.session.todos_by_agent = p.todos_by_agent;
            state.session.expanded_view = p.expanded_view;
            state.session.verification_nudge_pending = p.verification_nudge_pending;

            // Update the "all completed" anchor. Set when the
            // transition fires; clear if any non-completed task
            // resurfaces. Do nothing when the list is empty so the
            // anchor doesn't latch on a vacuous state.
            let all_completed = !state.session.plan_tasks.is_empty()
                && state
                    .session
                    .plan_tasks
                    .iter()
                    .all(|t| matches!(t.status, coco_types::TaskListStatus::Completed));
            if !all_completed {
                state.ui.ephemeral.tasks_all_completed_since_ms = None;
            } else if state.ui.ephemeral.tasks_all_completed_since_ms.is_none() {
                state.ui.ephemeral.tasks_all_completed_since_ms = Some(now);
            }
            true
        }
        ServerNotification::PlanApprovalRequested(p) => {
            let prompt = crate::state::PlanApprovalPromptState::new(
                p.request_id,
                p.from,
                p.plan_file_path,
                p.plan_content,
            );
            state.ui.push_prompt(PanePromptState::PlanApproval(prompt));
            true
        }
        ServerNotification::AgentsKilled(p) => {
            state
                .ui
                .add_toast(Toast::info(format!("{} agents killed", p.count)));
            true
        }

        // === Model ===
        ServerNotification::ModelFallbackStarted(p) => {
            state.session.model_fallback_banner =
                Some(format!("{} → {}", p.from_model, p.to_model));
            state.session.model = p.to_model;
            state.ui.add_toast(Toast::warning(
                t!(
                    "toast.fallback_short",
                    summary = state.session.model_fallback_banner.as_deref().unwrap_or("")
                )
                .to_string(),
            ));
            true
        }
        ServerNotification::ModelFallbackCompleted => {
            state.session.model_fallback_banner = None;
            true
        }
        ServerNotification::ModelRoleChanged(p) => {
            // Fold the binding into `model_by_role` (one entry per role
            // independent of provider/model_id so the picker's "current"
            // marker always points to the right row).
            state.session.model_by_role.insert(
                p.role,
                crate::state::ModelBinding {
                    model_id: p.model_id.clone(),
                    provider: p.provider.clone(),
                    context_window: p.context_window,
                    effort: p.effort,
                },
            );
            // Mirror Main role into the legacy `model`/`provider`/
            // `thinking_effort` fields the status bar reads directly.
            // Non-Main roles only affect the picker view.
            if p.role == coco_types::ModelRole::Main {
                state.session.model = p.model_id.clone();
                state.session.provider = p.provider.clone();
                state.session.thinking_effort =
                    p.effort.unwrap_or(coco_types::ReasoningEffort::Auto);
            }
            true
        }
        ServerNotification::FastModeChanged { active } => {
            state.session.fast_mode = active;
            // No toast — the status bar reflects fast-mode state.
            true
        }

        // === Permission ===
        ServerNotification::PermissionModeChanged(p) => {
            state.session.permission_mode = p.mode;
            // Route the capability gate so the TUI state + Shift+Tab
            // cycle reflect the session's current authorization state.
            // Without this, `session.bypass_permissions_available`
            // stayed at its init-time default and no client could ever
            // cycle into BypassPermissions via the gate.
            state.session.bypass_permissions_available = p.bypass_available;
            true
        }

        // === Prompt ===
        ServerNotification::PromptSuggestion { suggestions } => {
            state.session.prompt_suggestions = suggestions;
            true
        }

        // === System ===
        ServerNotification::Error(p) => {
            // Retryable errors auto-recover; keep them as ephemeral toasts.
            // Non-retryable errors escalate to a modal state so the user
            // must acknowledge before continuing (PR-F1 P0).
            state.ui.add_toast(Toast::error(p.message.clone()));
            if !p.retryable {
                let body = crate::widgets::error_dialog::error_body(&p);
                state.ui.show_modal(ModalState::Error(body));
            }
            true
        }
        ServerNotification::RateLimit(p) => {
            state.session.rate_limit_info = Some(RateLimitInfo {
                remaining: p.remaining,
                reset_at: p.reset_at,
                provider: p.provider,
            });
            if p.remaining == Some(0) {
                state
                    .ui
                    .add_toast(Toast::warning(t!("toast.rate_limited").to_string()));
            }
            true
        }
        // No-op: heartbeat has no UI effect.
        ServerNotification::KeepAlive { .. } => false,

        // === IDE ===
        // Latest-wins replacement — widgets read the stored payload when rendering.
        ServerNotification::IdeSelectionChanged(p) => {
            state.session.ide_selection = Some(p);
            true
        }
        ServerNotification::IdeDiagnosticsUpdated(p) => {
            state.session.ide_diagnostics = Some(p);
            true
        }

        // === Queue ===
        ServerNotification::QueueStateChanged { queued } => {
            // Reconciliation safety net: if per-item `CommandDequeued`
            // events were lost (channel saturation, fork dispatch
            // missed an emit) the engine's authoritative count clamps
            // the display so it never drifts to a stale, infinitely-
            // growing list.
            state
                .session
                .queued_commands
                .truncate(queued.max(0) as usize);
            true
        }
        ServerNotification::CommandQueued {
            id,
            preview,
            editable,
        } => {
            state
                .session
                .queued_commands
                .push_back(crate::state::session::QueuedCommandDisplay {
                    id,
                    preview,
                    editable,
                });
            true
        }
        ServerNotification::CommandDequeued { id } => {
            // Match by id so priority reordering between enqueue and
            // drain doesn't cause us to remove the wrong preview. Falls
            // back to pop_front if the id isn't tracked locally — that
            // happens when a producer emits CommandQueued through a
            // path the TUI didn't observe (e.g. SDK batch enqueues
            // before the TUI subscribed).
            if let Some(pos) = state
                .session
                .queued_commands
                .iter()
                .position(|q| q.id == id)
            {
                state.session.queued_commands.remove(pos);
            } else if !state.session.queued_commands.is_empty() {
                state.session.queued_commands.pop_front();
            }
            true
        }

        // === Rewind ===
        ServerNotification::RewindCompleted(p) => {
            // Rewind from protocol layer (vs TuiOnlyEvent::RewindCompleted
            // which carries the target_message_id). Protocol-level rewind
            // carries restored_files count; TUI toast only.
            let msg = if p.restored_files > 0 {
                t!("toast.rewound_files", count = p.restored_files).to_string()
            } else {
                t!("toast.conversation_rewound").to_string()
            };
            state.ui.add_toast(Toast::success(msg));
            true
        }
        ServerNotification::RewindFailed { error } => {
            state.ui.add_toast(Toast::error(
                t!("toast.rewind_failed_short", error = error.as_str()).to_string(),
            ));
            true
        }

        // === Cost ===
        ServerNotification::CostWarning(p) => {
            // Budget breach is a decision point — route through the modal
            // `CostWarning` state (already defined) so users can stop
            // or continue explicitly. Keep the toast for the event log.
            let toast_msg = format!(
                "Cost: ${:.2} / ${:.2} threshold",
                p.current_cost_cents as f64 / 100.0,
                p.threshold_cents as f64 / 100.0
            );
            state.ui.add_toast(Toast::warning(toast_msg));
            state.ui.push_prompt(PanePromptState::CostWarning(
                crate::state::CostWarningPromptState {
                    current_cost_cents: p.current_cost_cents,
                    threshold_cents: p.threshold_cents,
                },
            ));
            true
        }

        // === Sandbox ===
        ServerNotification::SandboxStateChanged(p) => {
            state.session.sandbox_active = p.active;
            true
        }
        ServerNotification::SandboxViolationsDetected { count } => {
            // Non-blocking count surface: violations render as an expandable
            // count view, not a blocking modal — a per-burst modal would
            // interrupt the turn repeatedly. A toast keeps the user informed;
            // the model also sees the details via `<sandbox_violations>`.
            state.ui.add_toast(Toast::error(format!(
                "Sandbox blocked {count} {}",
                if count == 1 {
                    "violation"
                } else {
                    "violations"
                }
            )));
            true
        }

        // === Agent ===
        ServerNotification::AgentsRegistered { agents } => {
            if !agents.is_empty() {
                state
                    .ui
                    .add_toast(Toast::info(format!("{} agents registered", agents.len())));
            }
            true
        }

        // === Hook ===
        ServerNotification::HookStarted(p) => {
            state.session.active_hooks.push(HookEntry {
                hook_id: p.hook_id,
                hook_name: p.hook_name,
                status: HookEntryStatus::Running,
                output: None,
            });
            true
        }
        ServerNotification::HookProgress(p) => {
            if let Some(hook) = state
                .session
                .active_hooks
                .iter_mut()
                .find(|h| h.hook_id == p.hook_id)
                && !p.stdout.is_empty()
            {
                hook.output = Some(p.stdout);
            }
            true
        }
        ServerNotification::HookResponse(p) => {
            if let Some(hook) = state
                .session
                .active_hooks
                .iter_mut()
                .find(|h| h.hook_id == p.hook_id)
            {
                hook.status = match p.outcome {
                    coco_types::HookOutcomeStatus::Success => HookEntryStatus::Completed,
                    coco_types::HookOutcomeStatus::Error
                    | coco_types::HookOutcomeStatus::Cancelled => HookEntryStatus::Failed,
                };
                hook.output = Some(p.output);
            }
            true
        }

        // === Worktree ===
        ServerNotification::WorktreeEntered(p) => {
            state.session.worktree_path = Some(p.worktree_path);
            true
        }
        ServerNotification::WorktreeExited(_) => {
            state.session.worktree_path = None;
            true
        }

        // === Summarize ===
        ServerNotification::SummarizeCompleted(_) => {
            state
                .ui
                .add_toast(Toast::info(t!("toast.summarize_complete").to_string()));
            true
        }
        ServerNotification::SummarizeFailed { error } => {
            let msg = t!("toast.summarize_failed_short", error = error.as_str()).to_string();
            state.ui.add_toast(Toast::error(msg.clone()));
            let body =
                crate::widgets::error_dialog::format_error_body(&msg, Some("summarize"), false);
            state.ui.show_modal(ModalState::Error(body));
            true
        }

        // === Stream health ===
        ServerNotification::StreamStallDetected { .. } => {
            state.session.stream_stall = true;
            state.ui.add_toast(Toast::warning(
                t!("toast.stream_stall_detected").to_string(),
            ));
            true
        }
        ServerNotification::StreamWatchdogWarning { elapsed_secs } => {
            state.ui.add_toast(Toast::warning(
                t!("toast.stream_watchdog", secs = format!("{elapsed_secs:.0}")).to_string(),
            ));
            true
        }
        // No-op: usage tracked via TurnCompleted, not per-request.
        ServerNotification::StreamRequestEnd { .. } => false,

        // === Session state ===
        ServerNotification::SessionStateChanged { state: new_state } => {
            state.session.session_state = new_state;
            true
        }

        // === P2 additions ===
        ServerNotification::LocalCommandOutput(p) => {
            const MAX_LOCAL_OUTPUT: usize = 50;
            state
                .session
                .local_command_output
                .push_back(p.content.to_string());
            while state.session.local_command_output.len() > MAX_LOCAL_OUTPUT {
                state.session.local_command_output.pop_front();
            }
            true
        }
        ServerNotification::FilesPersisted(p) => {
            let count = p.files.len();
            let failed = p.failed.len();
            let msg = if failed > 0 {
                format!("{count} files persisted, {failed} failed")
            } else {
                format!("{count} files persisted")
            };
            state.ui.add_toast(Toast::info(msg));
            true
        }
        ServerNotification::ElicitationComplete(p) => {
            // No elicitation dialog UI exists yet, so there is no matching
            // prompt to dismiss. Unconditionally calling `dismiss_prompt()`
            // would close an UNRELATED active prompt (permission / plan / MCP
            // approval). The correct approach is to match (server, elicitation_id)
            // against the queued elicitation and dismiss only that entry; until
            // the dialog lands we ignore the notification rather than dismiss the
            // wrong prompt.
            tracing::debug!(
                server = %p.mcp_server_name,
                elicitation_id = %p.elicitation_id,
                "ElicitationComplete ignored (no elicitation dialog to match)"
            );
            true
        }
        ServerNotification::ToolUseSummary(p) => {
            // Side-cache the summary for the tool batch — UI-only
            // overlay polish, intentionally NOT written to
            // `MessageHistory` (I-3: UI-only state stays UI-only).
            // Anchor on the first `preceding_tool_use_id` so the
            // renderer can attach the label to the assistant turn
            // that initiated the batch.
            match p.preceding_tool_use_ids.first() {
                Some(anchor) => {
                    tracing::debug!(
                        target: "coco_tui::tool_use_summary",
                        anchor_call_id = %anchor,
                        tool_count = p.preceding_tool_use_ids.len(),
                        summary_chars = p.summary.len(),
                        cache_size_after = state.session.tool_group_summaries.len() + 1,
                        "tool-use summary cached",
                    );
                    state
                        .session
                        .tool_group_summaries
                        .insert(anchor.clone(), p.summary);
                }
                None => {
                    tracing::warn!(
                        target: "coco_tui::tool_use_summary",
                        summary_chars = p.summary.len(),
                        "tool-use summary dropped: empty preceding_tool_use_ids \
                         (engine produced a summary with no batch anchor)",
                    );
                }
            }
            true
        }
        ServerNotification::ToolProgress(_) => {
            // Agent progress heartbeat. Returning `true` requests a redraw so
            // the activity strip's elapsed badge keeps ticking; the badge is
            // derived from `ToolExecution.started_at` and the args preview is
            // stamped once at `start_tool`, so there is no state to mutate
            // here. (This previously clobbered `description` with a redundant
            // `"{n}s"` echo that duplicated the elapsed badge.)
            true
        }

        // === Plugins ===
        // Surfaces the "Plugins changed. Run /reload-plugins to activate."
        // banner; the user must run `/reload-plugins` explicitly. Uses
        // the info toast (lightest priority).
        ServerNotification::PluginsChanged { reason: _ } => {
            state.ui.add_toast(Toast::info(
                "Plugins changed. Run /reload-plugins to activate.".to_string(),
            ));
            true
        }

        // === History lifecycle ===
        //
        // Engine MessageHistory authoritative-mutation events. These
        // feed `session.transcript`, which the renderer pipeline reads
        // exclusively.
        ServerNotification::MessageAppended { message, .. } => {
            // Plan §6.4 atomic finalize: when the appended message is
            // the assistant push that just ended the in-flight stream,
            // the cell now owns the content — drop the overlay so the
            // same text does not render twice for one frame. Subsequent
            // stream deltas inside the same turn re-create the
            // StreamingState lazily via `get_or_insert_with` in
            // `stream.rs`.
            //
            // Single-agent model: every assistant push is the current
            // stream. A future parallel-stream world would need an
            // anchor UUID on StreamingState + an engine-side pre-
            // allocated assistant UUID; no emitter for that exists
            // today so the role check is the correct minimal fix.
            let is_assistant = matches!(message.as_ref(), coco_messages::Message::Assistant(_));
            // D4: stamp any pending tool_executions whose `call_id`
            // matches a `ToolCall` content block in this assistant
            // message. Stamps the parent message UUID so the
            // MessageTruncated handler can retain in-flight executions
            // whose anchor survives the truncate.
            if is_assistant {
                state
                    .session
                    .stamp_tool_executions_with_assistant_uuid(&message);
                // Persist the raw markdown body for `/copy` and the
                // rewind-overlay preview. `record_agent_markdown` is invoked
                // on every assistant message append. The text extractor pulls
                // plain `AssistantContent::Text` parts only; reasoning / tool
                // calls / files are intentionally skipped.
                let text = coco_messages::wrapping::extract_text_from_message(message.as_ref());
                state.session.record_agent_markdown(&text);
            }
            state.session.transcript.on_message_appended(message);
            if is_assistant {
                state.ui.streaming = None;
            }
            true
        }
        ServerNotification::MessageTruncated { keep_count, .. } => {
            let n = keep_count.max(0) as usize;
            let cells_before = state.session.transcript.len();
            let reasoning_before = state.session.reasoning_metadata.len();
            let tool_widgets_before = state.session.tool_executions.len();
            state.session.transcript.on_message_truncated(n);
            // D4: drop only tool overlays whose anchor message no
            // longer survives. Unstamped executions (mid-stream, no
            // committed assistant message yet) are kept — they belong
            // to the live turn the user is interacting with. The
            // streaming overlay is the live-turn anchor and is always
            // cleared because truncate semantically ends the live turn.
            let surviving_uuids: std::collections::HashSet<uuid::Uuid> = state
                .session
                .transcript
                .cells()
                .iter()
                .map(|c| c.message_uuid)
                .collect();
            state
                .session
                .retain_tool_executions_for_messages(&surviving_uuids);
            // Prune side-caches keyed by message uuid so they don't
            // outlive their anchor message. `tool_group_summaries` is
            // keyed by `preceding_tool_use_id` (call id) so it stays;
            // it'll be cleared on session reset.
            state
                .session
                .retain_reasoning_metadata_for_messages(&surviving_uuids);
            state.ui.streaming = None;
            tracing::info!(
                target: "coco_tui::history",
                keep_count = n,
                cells_before,
                cells_after = state.session.transcript.len(),
                reasoning_cache_dropped = reasoning_before
                    .saturating_sub(state.session.reasoning_metadata.len()),
                tool_widgets_dropped = tool_widgets_before
                    .saturating_sub(state.session.tool_executions.len()),
                "MessageTruncated applied",
            );
            true
        }
        ServerNotification::SessionResetForResume { session_id, .. } => {
            tracing::info!(
                target: "coco_tui::history",
                new_session_id = %session_id,
                cells_cleared = state.session.transcript.len(),
                tool_widgets_cleared = state.session.tool_executions.len(),
                tool_summaries_cleared = state.session.tool_group_summaries.len(),
                reasoning_cache_cleared = state.session.reasoning_metadata.len(),
                "SessionResetForResume",
            );
            clear_session_boundary_state(state);
            state.session.transcript.on_session_reset();
            // Conversation id rotates on resume so prompt-cache keys
            // do not collide with the prior run's break points.
            state.session.conversation_id = Some(session_id);
            true
        }
        ServerNotification::ReasoningMetadataAttached(p) => {
            let Ok(uuid) = uuid::Uuid::parse_str(&p.message_uuid) else {
                tracing::warn!(
                    target: "coco_tui::reasoning",
                    message_uuid = %p.message_uuid,
                    "ReasoningMetadataAttached: invalid UUID, dropping",
                );
                return true;
            };
            tracing::debug!(
                target: "coco_tui::reasoning",
                %uuid,
                reasoning_tokens = p.reasoning_tokens,
                duration_ms = ?p.duration_ms,
                "stamping reasoning metadata in side-cache (event-driven)",
            );
            state.session.insert_reasoning_metadata(
                uuid,
                crate::state::session::ReasoningMetadata {
                    duration_ms: p.duration_ms,
                    reasoning_tokens: p.reasoning_tokens,
                },
            );
            true
        }
        ServerNotification::HistoryReplaced { messages, .. } => {
            // Bulk resume hydration: a single replace instead of N
            // MessageAppended events. UI-only side-caches that anchor
            // on message uuids get cleared because the new transcript
            // overwrites the old.
            tracing::info!(
                target: "coco_tui::history",
                incoming = messages.len(),
                cells_before = state.session.transcript.len(),
                "HistoryReplaced (bulk hydration)",
            );
            clear_session_boundary_state(state);
            state
                .session
                .transcript
                .replace_from_messages(messages.as_slice());
            tracing::debug!(
                target: "coco_tui::history",
                cells_after = state.session.transcript.len(),
                "HistoryReplaced applied",
            );
            true
        }
    }
}

/// Push (or no-op dedupe) a `SubagentInstance` row for the given
/// [`TaskStartedParams`]. Teammate-only metadata (`agent_name`,
/// `team_name`, `color`) is only consulted for [`SubagentKind::Teammate`];
/// BgAgent rows leave those `None` because their wire payload does too.
/// Recover a subagent's declared `subagent_type` (Explore / Plan / Review / …)
/// from the spawning Agent tool call's input, located by `tool_use_id` across
/// the committed transcript cells. Mirrors the lookup the transcript header
/// uses (`cells_renderer::render_tool_call_header`) so the panel badge and the
/// committed cell agree. `None` until the issuing assistant turn is committed.
fn declared_subagent_type(state: &AppState, tool_use_id: &str) -> Option<String> {
    state
        .session
        .transcript
        .cells()
        .iter()
        .find_map(|cell| {
            crate::transcript::derive::extract_tool_call_input(cell.source.as_ref(), tool_use_id)
        })
        .as_ref()
        .and_then(|input| input.get("subagent_type"))
        .and_then(serde_json::Value::as_str)
        .filter(|ty| !ty.is_empty())
        .map(str::to_string)
}

fn ensure_subagent_row(state: &mut AppState, kind: SubagentKind, p: &TaskStartedParams) {
    if state
        .session
        .subagents
        .iter()
        .any(|a| a.agent_id == p.task_id)
    {
        return;
    }
    let (agent_type, color, team_name) = match kind {
        SubagentKind::Subagent => {
            // `TaskStartedParams` doesn't surface the BgAgent's declared
            // agent_type (Explore / Plan / Review / …) on the wire. Recover
            // it from the spawning Agent tool call's `subagent_type` input
            // (keyed by `tool_use_id`) — the same source the committed
            // transcript header reads — so the badge is correct from the
            // first frame instead of flipping `local_agent → Explore` only
            // once the first `TaskProgress` lands. Falls back to the wire
            // literal when the ToolUse cell isn't committed yet; the
            // `TaskProgress` handler still corrects it in that race.
            let declared = p
                .tool_use_id
                .as_deref()
                .and_then(|tuid| declared_subagent_type(state, tuid));
            (
                declared.unwrap_or_else(|| task_type_wire::LOCAL_AGENT.to_string()),
                None,
                None,
            )
        }
        SubagentKind::Teammate => {
            // `agent_name` is the bare name; fall back to the
            // `name@team` task_id when older emitters didn't
            // populate it.
            (
                p.agent_name.clone().unwrap_or_else(|| p.task_id.clone()),
                // Parse the wire color string into the typed enum at the
                // protocol boundary (unknown values collapse to `None`).
                p.color.as_deref().and_then(|c| c.parse().ok()),
                p.team_name.clone().filter(|s| !s.is_empty()),
            )
        }
    };
    let started_at_ms = state.clock.now_ms();
    state.session.subagents.push(SubagentInstance {
        kind,
        agent_id: p.task_id.clone(),
        agent_type,
        description: p.description.clone(),
        status: SubagentStatus::Running,
        color,
        team_name,
        started_at_ms: Some(started_at_ms),
        last_tool_name: None,
        tool_count: 0,
        total_tokens: 0,
        is_backgrounded: false,
        recent_activities: Vec::new(),
        final_message: None,
        completed_at_ms: None,
        cost_usd: 0.0,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
    });
}

/// First line of `text`, char-truncated to `max_chars - 1` glyphs plus
/// an ellipsis when truncation actually fires. The `-1` accounts for the
/// ellipsis so the final visible width never exceeds `max_chars`.
fn preview_summary(text: &str, max_chars: usize) -> String {
    let trimmed = text.lines().next().unwrap_or(text);
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn token_usage_from_session_snapshot(
    snapshot: &coco_types::SessionUsageSnapshot,
) -> crate::state::session::TokenUsage {
    crate::state::session::TokenUsage {
        input_tokens: snapshot.totals.input_tokens,
        output_tokens: snapshot.totals.output_tokens,
        reasoning_tokens: 0,
        cache_read_tokens: snapshot.totals.cache_read_input_tokens,
        cache_creation_tokens: snapshot.totals.cache_creation_input_tokens,
    }
}

#[cfg(test)]
#[path = "protocol.test.rs"]
mod tests;
