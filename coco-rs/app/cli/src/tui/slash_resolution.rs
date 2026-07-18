use super::*;
/// Body of `UserCommand::SubmitInput` extracted into an async fn so
/// it can be `tokio::spawn`ed. The dispatch loop stores the
/// `JoinHandle` in `active_turn` and continues to recv the next
/// command — letting `Interrupt` / `Compact` /
/// `Rewind` / `Shutdown` reach their arms while the engine runs.
/// All session-scoped Arcs are read out of `runtime` inside the body —
/// the only data piped in are the per-turn user inputs, the cancel
/// token, the cross-turn `title_gen_attempted` latch, and the snapshot
/// of `session_id` taken on the dispatcher side (so the title-gen path
/// uses the same id the rest of the turn observed, not a later
/// `/clear`-regenerated one).
/// Outcome of slash-command resolution against `runtime.command_registry`.
/// `dispatch_slash_command` is the single source of truth for routing
/// `/foo` regardless of whether the user typed it (`SubmitInput`) or
/// picked it from the palette (`ExecuteSkill`).
pub(super) enum SlashOutcome {
    /// Command consumed locally (Text / Compact / OpenDialog / Skip).
    /// The caller should NOT run the engine.
    Handled,
    /// Re-feed `content` into the engine as the user message
    /// (Prompt / InjectPrompt). For typed commands the original `/foo`
    /// is replaced with the rendered prompt body so the model sees the
    /// expansion, not the slash.
    RunEngine {
        content: String,
        metadata: Option<String>,
        thinking_level: Option<coco_types::ThinkingLevel>,
        model_runtime_source: Option<coco_inference::ModelRuntimeSource>,
    },
    /// A user-typed fork-mode skill (`/<name>` with `context: fork`).
    /// Unlike `RunEngine` (which re-queries the main model with the
    /// expanded body), this runs the skill as a subagent via the
    /// installed `SkillHandle` and injects only its result — mirroring
    /// `executeForkedSlashCommand`. `name` is the canonical skill name.
    RunForkSkill { name: String, args: String },
    /// No command with this name is registered. Caller should fall
    /// through to the existing path (model receives raw text).
    NotFound,
    /// Trigger the same flow as `UserCommand::Compact`. Emitted when
    /// the slash dispatcher detects `COMPACT_SENTINEL` (palette path)
    /// or intercepts `/compact` / `/compact <args>` directly. The agent
    /// driver sends the sentinel through local AppServer `turn/start`, whose
    /// compact shortcut runs the actual summarization task.
    TriggerCompact { custom_instructions: Option<String> },
    /// Trigger the clear flow for `/clear`. The agent driver swaps in a
    /// fresh local AppServer-backed runtime and emits the TUI reset event.
    TriggerClear,
    /// Trigger auto-memory consolidation (when the runtime has a
    /// `MemoryRuntime`). Emitted when the dispatcher sees `DREAM_SENTINEL`.
    TriggerDream,
    /// Trigger a session-memory force update (9-section). Emitted when
    /// the dispatcher sees `SUMMARY_SENTINEL`.
    TriggerSummary,
    /// Render the live multi-provider session cost. Emitted when the
    /// dispatcher sees `COST_SENTINEL`; the runner asks local AppServer
    /// `session/cost` for the live usage snapshot and formatted report.
    ShowCost,
    /// Render the live session status (model / permission mode / thinking /
    /// plan mode / MCP servers). Emitted on `STATUS_SENTINEL`; the runner
    /// asks local AppServer `session/status`.
    ShowStatus,
    /// Install, clear, or show the session-scoped `/goal` Stop hook.
    TriggerGoal {
        request: coco_commands::GoalCommandRequest,
    },
    /// Rename the current session. `Explicit (name)` uses the
    /// caller-supplied name verbatim; `Auto` directs the dispatcher
    /// to derive a kebab-case name via the `ModelRole::Fast`
    /// resolver. Either way the runner persists via
    /// [`coco_session::SessionManager::set_title`] (which writes
    /// both `CustomTitle` and `AgentName`) and patches the PID
    /// registry so `coco ps` reflects the new name live.
    TriggerRename {
        request: coco_commands::ParsedRename,
    },
    /// Toggle a tag on the current session through local AppServer
    /// `session/toggleTag`.
    TriggerTag { tag: String },
    /// Push `path` onto the live `ToolAppState.permissions.additional_dirs`
    /// base so the next batch's permission context sees the wider scope.
    TriggerAddDir { path: String },
    /// Open a concrete session plan file through the same external
    /// editor terminal handoff used by prompt and memory editing.
    TriggerOpenPlanEditor { path: std::path::PathBuf },
    /// Open a local ephemeral sidechat child, optionally with a first prompt.
    TriggerBtw {
        request: coco_commands::handlers::btw::BtwRequest,
        images: Vec<coco_types::QueuedCommandEditImage>,
    },
    /// Rebuild the slash-command registry from disk and atomically
    /// swap. Triggered by `/reload-plugins`.
    TriggerReloadPlugins,
    /// Reload the live `HookRegistry` from the latest `RuntimeConfig`
    /// snapshot. Triggered by `/hooks reload`.
    /// Slash commands run only at turn boundaries (the dispatch loop
    /// `drain_active_turn`s before invoking them), so
    /// PreToolUse/PostToolUse for an in-flight call cannot see
    /// different hook sets.
    TriggerReloadHooks,
}

pub(super) fn slash_unavailable_in_session_message(name: &str) -> String {
    format!("/{name} isn't available in this session.")
}

pub(super) const SIDECHAT_SLASH_POLICY_MESSAGE: &str =
    "Sidechat supports only /compact and /context. Press Ctrl+C to return to main.";

/// Split `/<name> <args>` into ` (name, args)`. Returns `None` when
/// `text` does not start with `/` or has no name. Whitespace-trimmed.
/// Convert a `coco_context::DiffStats` to the wire payload variant.
/// Centralised so the single-row and batch paths emit identically.
pub(super) fn diff_stats_to_payload(
    stats: coco_context::DiffStats,
) -> coco_types::RewindDiffStatsPayload {
    let file_paths: Vec<String> = stats
        .files_changed
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    coco_types::RewindDiffStatsPayload {
        insertions: stats.insertions,
        deletions: stats.deletions,
        file_paths,
    }
}

pub(super) fn parse_slash_command(text: &str) -> Option<(&str, &str)> {
    let stripped = text.trim().strip_prefix('/')?;
    if stripped.is_empty() {
        return None;
    }
    Some(match stripped.split_once(char::is_whitespace) {
        Some((name, rest)) => (name, rest.trim_start()),
        None => (stripped, ""),
    })
}

pub(super) async fn prepare_external_editor_request(
    pending_editor_requests: &mut HashMap<String, PendingEditorRequest>,
    request: PendingEditorRequest,
    event_tx: &mpsc::Sender<CoreEvent>,
) {
    let request_id = uuid::Uuid::new_v4().to_string();
    pending_editor_requests.insert(request_id.clone(), request);
    let _ = event_tx
        .send(CoreEvent::Tui(TuiOnlyEvent::ExternalEditorPrepare {
            request_id,
        }))
        .await;
}

/// Decision-tree classifier for sentinel-prefixed handler output.
/// Pure, no side-effects — used by `dispatch_slash_command` to decide
/// whether the Text result actually carries a request to fire a real
/// feature (compact / dream / summary / rename / tag). Extracted as a
/// free function so the routing logic is testable without a full
/// `SessionRuntime`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SentinelTrigger {
    Compact {
        custom_instructions: Option<String>,
    },
    Dream,
    Summary,
    Cost,
    Status,
    Goal {
        request: coco_commands::GoalCommandRequest,
    },
    Rename {
        request: coco_commands::ParsedRename,
    },
    Tag {
        tag: String,
    },
    AddDir {
        path: String,
    },
    ReloadPlugins,
    ReloadHooks,
}

pub(super) fn classify_sentinel_trigger(text: &str) -> Option<SentinelTrigger> {
    use coco_commands::handlers::compact::COMPACT_SENTINEL;
    use coco_commands::handlers::compact::parse_compact_sentinel;
    use coco_commands::handlers::dream::DREAM_SENTINEL;
    use coco_commands::handlers::dream::parse_dream_sentinel;
    use coco_commands::handlers::summary::SUMMARY_SENTINEL;
    use coco_commands::handlers::summary::parse_summary_sentinel;
    if text.starts_with(COMPACT_SENTINEL) {
        let req = parse_compact_sentinel(text)?;
        let trimmed = req.custom_instructions.trim();
        let custom_instructions = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
        return Some(SentinelTrigger::Compact {
            custom_instructions,
        });
    }
    if text.starts_with(DREAM_SENTINEL) && parse_dream_sentinel(text).is_some() {
        return Some(SentinelTrigger::Dream);
    }
    if text.starts_with(SUMMARY_SENTINEL) && parse_summary_sentinel(text).is_some() {
        return Some(SentinelTrigger::Summary);
    }
    if text.starts_with(coco_commands::handlers::cost::COST_SENTINEL)
        && coco_commands::handlers::cost::parse_cost_sentinel(text).is_some()
    {
        return Some(SentinelTrigger::Cost);
    }
    if text.starts_with(coco_commands::STATUS_SENTINEL)
        && coco_commands::parse_status_sentinel(text).is_some()
    {
        return Some(SentinelTrigger::Status);
    }
    if text.starts_with(coco_commands::GOAL_SENTINEL)
        && let Some(request) = coco_commands::parse_goal_sentinel(text)
    {
        return Some(SentinelTrigger::Goal { request });
    }
    if text.starts_with(coco_commands::RENAME_SENTINEL)
        && let Some(request) = coco_commands::parse_rename_sentinel(text)
    {
        return Some(SentinelTrigger::Rename { request });
    }
    if text.starts_with(coco_commands::TAG_SENTINEL)
        && let Some(tag) = coco_commands::parse_tag_sentinel(text)
    {
        return Some(SentinelTrigger::Tag { tag });
    }
    if text.starts_with(coco_commands::ADD_DIR_SENTINEL)
        && let Some(path) = coco_commands::parse_add_dir_sentinel(text)
    {
        return Some(SentinelTrigger::AddDir { path });
    }
    if text.starts_with(coco_commands::RELOAD_PLUGINS_SENTINEL)
        && coco_commands::parse_reload_plugins_sentinel(text).is_some()
    {
        return Some(SentinelTrigger::ReloadPlugins);
    }
    if text.starts_with(coco_commands::RELOAD_HOOKS_SENTINEL)
        && coco_commands::parse_reload_hooks_sentinel(text).is_some()
    {
        return Some(SentinelTrigger::ReloadHooks);
    }
    None
}

/// Resolve `/<name> <args>` through the registry and route the result.
/// What's left for the caller to do after [`handle_slash_outcome`]
/// has processed an outcome.
/// The 9 `SlashOutcome::Trigger*` variants and `Handled` all fold to
/// [`SlashFollowup::Done`] inside the helper — caller has nothing
/// further to do (TUI may `continue`, palette / SDK may simply
/// no-op). The remaining two cases differ per call site and are
/// surfaced as variants here so each site renders the right
/// notification / continuation.
#[derive(Debug)]
pub(super) enum SlashFollowup {
    /// Outcome fully handled inside the helper. Caller continues.
    Done,
    /// Registry / palette did not recognise the command. Caller
    /// decides: typed input falls through to the LLM as raw text;
    /// palette logs; SDK emits `SlashCommandStatusKind::NoHandler`.
    NotFound,
    /// Command expanded to a model prompt. Caller spawns a turn
    /// (palette / SDK) or substitutes `effective_content` (typed input).
    RunEngine {
        content: String,
        metadata: Option<String>,
        thinking_level: Option<coco_types::ThinkingLevel>,
        model_runtime_source: Option<coco_inference::ModelRuntimeSource>,
    },
}

pub(super) struct SlashEnginePrompt {
    pub(super) content: String,
    pub(super) metadata: Option<String>,
    pub(super) thinking_level: Option<coco_types::ThinkingLevel>,
    pub(super) model_runtime_source: Option<coco_inference::ModelRuntimeSource>,
}

pub(super) struct LocalRuntimeControlContext<'a> {
    pub(super) current_session: &'a SharedSessionHandle,
    pub(super) runtime_reload_subscriptions: &'a Arc<Mutex<TuiRuntimeReloadSubscriptions>>,
    pub(super) turn_done_tx: &'a mpsc::Sender<uuid::Uuid>,
}

/// Process a [`SlashOutcome`] into a [`SlashFollowup`] for the
/// caller. Handles the trigger variants in one place so the dispatch
/// arms in `run_agent_loop` no longer triple-duplicate the same match.
pub(super) async fn handle_slash_outcome(
    outcome: SlashOutcome,
    session: &crate::session_runtime::SessionHandle,
    control_context: &LocalRuntimeControlContext<'_>,
    event_tx: &mpsc::Sender<CoreEvent>,
    active_turn: &Arc<Mutex<Option<ActiveTurn>>>,
    pending_editor_requests: &mut HashMap<String, PendingEditorRequest>,
    local_app_server_bridge: &mut coco_agent_host::app_server_host::AppServerLocalBridge,
) -> SlashFollowup {
    match outcome {
        SlashOutcome::Handled => SlashFollowup::Done,
        SlashOutcome::NotFound => SlashFollowup::NotFound,
        SlashOutcome::RunEngine {
            content,
            metadata,
            thinking_level,
            model_runtime_source,
        } => SlashFollowup::RunEngine {
            content,
            metadata,
            thinking_level,
            model_runtime_source,
        },
        SlashOutcome::RunForkSkill { name, args } => {
            run_fork_skill(session, event_tx, &name, &args, active_turn).await;
            SlashFollowup::Done
        }
        SlashOutcome::TriggerCompact {
            custom_instructions,
        } => {
            run_manual_compact(
                session,
                event_tx,
                local_app_server_bridge,
                custom_instructions,
                active_turn,
                control_context.turn_done_tx,
            )
            .await;
            SlashFollowup::Done
        }
        SlashOutcome::TriggerClear => {
            run_clear_conversation(
                session,
                control_context,
                active_turn,
                event_tx,
                local_app_server_bridge,
            )
            .await;
            SlashFollowup::Done
        }
        SlashOutcome::TriggerDream => {
            run_dream_consolidation(
                session,
                event_tx,
                local_app_server_bridge,
                active_turn,
                control_context.turn_done_tx,
            )
            .await;
            SlashFollowup::Done
        }
        SlashOutcome::TriggerSummary => {
            run_session_memory_force(
                session,
                event_tx,
                local_app_server_bridge,
                active_turn,
                control_context.turn_done_tx,
            )
            .await;
            SlashFollowup::Done
        }
        SlashOutcome::TriggerBtw { request, images } => {
            run_side_chat(
                session,
                event_tx,
                local_app_server_bridge,
                active_turn,
                control_context.turn_done_tx,
                request,
                &images,
            )
            .await;
            SlashFollowup::Done
        }
        SlashOutcome::ShowCost => {
            run_show_cost(session, event_tx, local_app_server_bridge).await;
            SlashFollowup::Done
        }
        SlashOutcome::ShowStatus => {
            run_show_status(session, event_tx, local_app_server_bridge).await;
            SlashFollowup::Done
        }
        SlashOutcome::TriggerGoal { request } => run_goal_command(session, event_tx, request).await,
        SlashOutcome::TriggerRename { request } => {
            run_session_rename(session, event_tx, local_app_server_bridge, request).await;
            SlashFollowup::Done
        }
        SlashOutcome::TriggerTag { tag } => {
            run_session_tag(session, event_tx, local_app_server_bridge, &tag).await;
            SlashFollowup::Done
        }
        SlashOutcome::TriggerAddDir { path } => {
            let _ = apply_session_add_directory(
                &path,
                session.session_id(),
                event_tx,
                local_app_server_bridge,
            )
            .await;
            SlashFollowup::Done
        }
        SlashOutcome::TriggerOpenPlanEditor { path } => {
            prepare_external_editor_request(
                pending_editor_requests,
                PendingEditorRequest::Plan { path },
                event_tx,
            )
            .await;
            SlashFollowup::Done
        }
        SlashOutcome::TriggerReloadPlugins => {
            run_reload_plugins(session, event_tx, local_app_server_bridge).await;
            SlashFollowup::Done
        }
        SlashOutcome::TriggerReloadHooks => {
            run_reload_hooks(session, event_tx, local_app_server_bridge).await;
            SlashFollowup::Done
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn process_idle_command_queue(
    session: &crate::session_runtime::SessionHandle,
    current_session: &SharedSessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_agent_host::app_server_host::AppServerLocalBridge,
    active_turn: &Arc<Mutex<Option<ActiveTurn>>>,
    pending_editor_requests: &mut HashMap<String, PendingEditorRequest>,
    title_gen_attempted: &Arc<RwLock<std::collections::HashSet<String>>>,
    turn_done_tx: &mpsc::Sender<uuid::Uuid>,
    runtime_reload_subscriptions: &Arc<Mutex<TuiRuntimeReloadSubscriptions>>,
) {
    if active_turn.lock().await.is_some() {
        return;
    }

    drain_queued_slash_commands(
        session,
        current_session,
        event_tx,
        local_app_server_bridge,
        active_turn,
        pending_editor_requests,
        title_gen_attempted,
        turn_done_tx,
        runtime_reload_subscriptions,
    )
    .await;

    if active_turn.lock().await.is_none() {
        spawn_command_queue_turn(
            session,
            event_tx,
            local_app_server_bridge,
            active_turn,
            turn_done_tx,
        )
        .await;
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn drain_queued_slash_commands(
    session: &crate::session_runtime::SessionHandle,
    current_session: &SharedSessionHandle,
    event_tx: &mpsc::Sender<CoreEvent>,
    local_app_server_bridge: &mut coco_agent_host::app_server_host::AppServerLocalBridge,
    active_turn: &Arc<Mutex<Option<ActiveTurn>>>,
    pending_editor_requests: &mut HashMap<String, PendingEditorRequest>,
    title_gen_attempted: &Arc<RwLock<std::collections::HashSet<String>>>,
    turn_done_tx: &mpsc::Sender<uuid::Uuid>,
    runtime_reload_subscriptions: &Arc<Mutex<TuiRuntimeReloadSubscriptions>>,
) {
    while let Some(cmd) = coco_agent_host::session_queue::dequeue_next_slash_command(session).await
    {
        let _ = event_tx
            .send(CoreEvent::Protocol(ServerNotification::CommandDequeued {
                id: cmd.id,
            }))
            .await;
        let _ = event_tx
            .send(CoreEvent::Protocol(ServerNotification::QueueStateChanged {
                queued: cmd.remaining_queued as i32,
            }))
            .await;

        let Some((name, args)) = parse_slash_command(&cmd.prompt) else {
            continue;
        };
        let images: Vec<coco_types::QueuedCommandEditImage> = cmd
            .images
            .into_iter()
            .map(|image| coco_types::QueuedCommandEditImage {
                media_type: image.media_type,
                data_base64: image.data_base64,
            })
            .collect();
        let outcome = dispatch_slash_command(
            name,
            args,
            session,
            current_session,
            event_tx,
            local_app_server_bridge,
            runtime_reload_subscriptions,
            &images,
        )
        .await;
        match outcome {
            SlashOutcome::Handled => {}
            SlashOutcome::NotFound => {
                emit_slash_status(
                    event_tx,
                    session.session_id(),
                    name,
                    args,
                    SlashCommandStatusKind::NoHandler,
                )
                .await;
            }
            SlashOutcome::RunEngine {
                content,
                metadata,
                thinking_level,
                model_runtime_source,
            } => {
                let session_id = session.session_id().clone();
                spawn_slash_run_engine_turn(
                    SlashEnginePrompt {
                        content,
                        metadata,
                        thinking_level,
                        model_runtime_source,
                    },
                    session,
                    event_tx,
                    local_app_server_bridge,
                    active_turn,
                    title_gen_attempted,
                    turn_done_tx,
                    &session_id,
                )
                .await;
                break;
            }
            SlashOutcome::RunForkSkill { name, args } => {
                run_fork_skill(session, event_tx, &name, &args, active_turn).await;
            }
            SlashOutcome::TriggerCompact {
                custom_instructions,
            } => {
                run_manual_compact(
                    session,
                    event_tx,
                    local_app_server_bridge,
                    custom_instructions,
                    active_turn,
                    turn_done_tx,
                )
                .await;
            }
            SlashOutcome::TriggerClear => {
                let control_context = LocalRuntimeControlContext {
                    current_session,
                    runtime_reload_subscriptions,
                    turn_done_tx,
                };
                run_clear_conversation(
                    session,
                    &control_context,
                    active_turn,
                    event_tx,
                    local_app_server_bridge,
                )
                .await;
            }
            SlashOutcome::TriggerDream => {
                run_dream_consolidation(
                    session,
                    event_tx,
                    local_app_server_bridge,
                    active_turn,
                    turn_done_tx,
                )
                .await;
            }
            SlashOutcome::TriggerSummary => {
                run_session_memory_force(
                    session,
                    event_tx,
                    local_app_server_bridge,
                    active_turn,
                    turn_done_tx,
                )
                .await;
            }
            SlashOutcome::TriggerBtw { request, images } => {
                run_side_chat(
                    session,
                    event_tx,
                    local_app_server_bridge,
                    active_turn,
                    turn_done_tx,
                    request,
                    &images,
                )
                .await;
            }
            SlashOutcome::ShowCost => {
                run_show_cost(session, event_tx, local_app_server_bridge).await;
            }
            SlashOutcome::ShowStatus => {
                run_show_status(session, event_tx, local_app_server_bridge).await;
            }
            SlashOutcome::TriggerGoal { request } => {
                if let SlashFollowup::RunEngine {
                    content,
                    metadata,
                    thinking_level,
                    model_runtime_source,
                } = run_goal_command(session, event_tx, request).await
                {
                    let session_id = session.session_id().clone();
                    spawn_slash_run_engine_turn(
                        SlashEnginePrompt {
                            content,
                            metadata,
                            thinking_level,
                            model_runtime_source,
                        },
                        session,
                        event_tx,
                        local_app_server_bridge,
                        active_turn,
                        title_gen_attempted,
                        turn_done_tx,
                        &session_id,
                    )
                    .await;
                    break;
                }
            }
            SlashOutcome::TriggerRename { request } => {
                run_session_rename(session, event_tx, local_app_server_bridge, request).await;
            }
            SlashOutcome::TriggerTag { tag } => {
                run_session_tag(session, event_tx, local_app_server_bridge, &tag).await;
            }
            SlashOutcome::TriggerAddDir { path } => {
                let _ = apply_session_add_directory(
                    &path,
                    session.session_id(),
                    event_tx,
                    local_app_server_bridge,
                )
                .await;
            }
            SlashOutcome::TriggerOpenPlanEditor { path } => {
                prepare_external_editor_request(
                    pending_editor_requests,
                    PendingEditorRequest::Plan { path },
                    event_tx,
                )
                .await;
            }
            SlashOutcome::TriggerReloadPlugins => {
                run_reload_plugins(session, event_tx, local_app_server_bridge).await;
            }
            SlashOutcome::TriggerReloadHooks => {
                run_reload_hooks(session, event_tx, local_app_server_bridge).await;
            }
        }
    }
}
