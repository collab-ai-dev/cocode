//! Production [`TurnRunner`] backed by [`coco_query::QueryEngine`].
//!
//! This is the bridge between the SDK dispatch layer (which knows only
//! about the `TurnRunner` trait) and the real agent loop. The CLI entry
//! point in `main.rs` constructs one of these per-process and hands it
//! to `SdkServer::with_turn_runner`.
//!
//! Scope:
//! - One QueryEngine per turn (fresh config). Multi-turn context is
//!   threaded forward via `TurnHandoff.history`: the runner locks
//!   the shared history, builds
//!   `prior_history + [create_user_message(prompt)]`, calls
//!   `run_with_messages`, and replaces the history with
//!   `result.final_messages` on completion.
//! - Forwards CoreEvents emitted by the engine directly onto the SDK
//!   server's `event_tx`. The server's notification forwarder then
//!   translates protocol events into JSON-RPC notifications on the wire.
//!
//! The SDK client drives the cadence via multiple `turn/start` calls
//! per session.

use std::pin::Pin;
use std::sync::Arc;

use coco_inference::ModelRuntimeSource;
use coco_messages::MessageHistory;
use coco_query::QueryEngineConfig;
use coco_types::CoreEvent;
use coco_types::ModelRole;
use coco_types::TurnStartParams;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::info;
use tracing::warn;

use crate::sdk_server::handlers::TurnHandoff;
use crate::sdk_server::handlers::TurnRunner;

/// `TurnRunner` implementation that spawns a fresh `QueryEngine` per
/// turn.
///
/// Holds a `SessionHandle` for the same per-session state container
/// the TUI runner uses. Per-turn engine assembly routes through
/// `runtime.build_engine_from_config(...)` so SDK and TUI share the
/// `with_*` install list.
pub struct QueryEngineRunner {
    session: crate::session_runtime::SessionHandle,
    /// Max internal agent turns (tool-use iterations) per SDK turn.
    /// `None` = unbounded unless `max_turns` is supplied in the request
    /// or `loop.max_turns` in settings.
    max_turns: Option<i32>,
    /// Optional system prompt. When None, the engine uses its default.
    system_prompt: Option<String>,
}

impl QueryEngineRunner {
    /// Build a runner from a pre-constructed [`SessionHandle`] (which
    /// already owns the client / tools / fallbacks / hook registry / all
    /// session subsystems).
    pub fn new(
        session: crate::session_runtime::SessionHandle,
        max_turns: Option<i32>,
        system_prompt: Option<String>,
    ) -> Self {
        Self {
            session,
            max_turns,
            system_prompt,
        }
    }
}

fn turn_start_images_to_tui(
    images: &[coco_types::QueuedCommandEditImage],
) -> Vec<coco_tui::ImageData> {
    use base64::Engine as _;

    images
        .iter()
        .filter_map(|image| {
            let bytes = match base64::engine::general_purpose::STANDARD
                .decode(image.data_base64.as_bytes())
            {
                Ok(bytes) => bytes,
                Err(error) => {
                    warn!(
                        media_type = %image.media_type,
                        error = %error,
                        "dropping invalid turn/start image payload"
                    );
                    return None;
                }
            };
            Some(coco_tui::ImageData {
                bytes,
                mime: if image.media_type.is_empty() {
                    "image/png".to_string()
                } else {
                    image.media_type.clone()
                },
            })
        })
        .collect()
}

fn create_slash_metadata_message(metadata: &str) -> coco_messages::Message {
    let attachment = coco_messages::AttachmentMessage::api(
        coco_types::AttachmentKind::SlashCommandMetadata,
        coco_messages::LlmMessage::user_text(metadata),
    );
    coco_messages::Message::Attachment(attachment)
}

impl TurnRunner for QueryEngineRunner {
    fn run_turn<'a>(
        &'a self,
        params: TurnStartParams,
        turn_id: coco_types::TurnId,
        handoff: TurnHandoff,
        event_tx: mpsc::Sender<CoreEvent>,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
        let mut prompt = params.prompt;
        let images = turn_start_images_to_tui(&params.images);
        let history_override = params.history_override;
        let slash_metadata = params.slash_metadata.clone();
        let model_selection_override = params.model_selection.clone();
        let permission_mode_override = params.permission_mode;
        let thinking_level_override = params.thinking_level;
        let system_prompt = self.system_prompt.clone();
        let max_turns = self.max_turns;
        let session = self.session.clone();
        let runtime = self.session.runtime().clone();
        let history_handle = handoff.history.clone();
        // Keep our own handle on the cancel token. The engine consumes
        // its copy; we still need to know post-run whether the user
        // requested an interrupt so the wire stream gets `turn/interrupted`
        // rather than `turn/failed`.
        let cancel_for_terminal = cancel.clone();
        Box::pin(async move {
            info!(
                session_id = %handoff.session_id,
                model = %handoff.model,
                cwd = %handoff.cwd,
                "QueryEngineRunner: run_turn"
            );

            // Resolve the permission mode. Priority:
            //   1. `params.permission_mode` (turn-scoped).
            //   2. `handoff.permission_mode` (session-scoped, set by
            //      `control/setPermissionMode`).
            //   3. `PermissionMode::default()`.
            let permission_mode = permission_mode_override
                .or(handoff.permission_mode)
                .unwrap_or_default();

            // Re-use the SessionRuntime's already-loaded `RuntimeConfig`
            // instead of re-running `RuntimeConfigBuilder::from_process`
            // per turn. The runtime's config is the canonical session-
            // scoped resolution (incl. CLI overrides + flag settings);
            // rebuilding from `from_process` would lose them and slow
            // every turn down by re-walking settings layers.
            let runtime_config = runtime.runtime_config.as_ref();
            // SDK turns honor the same settings-layered permission rules
            // as TUI / headless.
            let (allow_rules, deny_rules, ask_rules) =
                crate::permission_rule_loader::typed_permission_rules(&runtime_config.settings);
            let permission_rule_source_roots =
                crate::permission_rule_loader::permission_rule_source_roots(
                    &runtime_config.settings,
                    &runtime.original_cwd,
                );
            let current_engine_config = runtime.current_engine_config().await;
            let model_selection = model_selection_override;
            let model_runtime_source = model_selection
                .clone()
                .map(ModelRuntimeSource::Explicit)
                .unwrap_or(ModelRuntimeSource::Role(ModelRole::Main));
            let model_id = model_selection
                .as_ref()
                .map(|selection| selection.model_id.clone())
                .unwrap_or_else(|| current_engine_config.model_id.clone());
            let mut plan_mode_settings = runtime_config.settings.merged.plan_mode.clone();
            if let Some(instructions) = handoff.plan_mode_instructions.clone() {
                plan_mode_settings.custom_instructions = Some(instructions);
            }
            let config = QueryEngineConfig {
                model_id,
                permission_mode,
                permission_rule_source_roots: permission_rule_source_roots.clone(),
                // Request `max_turns` wins, else settings `loop.max_turns`,
                // else unbounded.
                max_turns: max_turns
                    .or(current_engine_config.max_turns)
                    .or(runtime_config.loop_config.max_turns),
                total_token_budget: current_engine_config
                    .total_token_budget
                    .or_else(|| runtime_config.loop_config.total_token_budget.map(i64::from)),
                prompt_cache: runtime
                    .model_runtimes()
                    .snapshot_for_source(model_runtime_source.clone())
                    .ok()
                    .is_some_and(|snapshot| snapshot.supports_prompt_cache)
                    .then(|| coco_types::PromptCacheConfig {
                        mode: coco_types::PromptCacheMode::Auto,
                        ttl: coco_types::CacheTtl::OneHour,
                        scope: None,
                        requested_betas: Default::default(),
                        skip_cache_write: false,
                    }),
                system_prompt: system_prompt
                    .or_else(|| current_engine_config.system_prompt.clone()),
                streaming_tool_execution: runtime_config.loop_config.enable_streaming_tools,
                session_id: handoff.session_id.clone(),
                tool_config: runtime_config.tool.clone(),
                sandbox_config: runtime_config.sandbox.clone(),
                sandbox_state: runtime.sandbox_state(),
                memory_config: runtime_config.memory.clone(),
                shell_config: runtime_config.shell.clone(),
                active_shell_tool: current_engine_config.active_shell_tool,
                shell_provider: current_engine_config.shell_provider.clone(),
                web_fetch_config: runtime_config.web_fetch.clone(),
                web_search_config: runtime_config.web_search.clone(),
                compact: runtime_config.compact.clone(),
                plan_mode_settings,
                thinking_level: thinking_level_override
                    .or(handoff.thinking_level.clone())
                    .or(current_engine_config.thinking_level.clone()),
                features: std::sync::Arc::new(runtime_config.features.clone()),
                skill_overrides: std::sync::Arc::new(runtime_config.skill_overrides.clone()),
                tool_overrides: runtime_config.tool_overrides.clone(),
                // Inherit `--include-hook-events` from the runtime's
                // stored engine config so SDK turns honour the flag the
                // session was started with.
                include_hook_events: current_engine_config.include_hook_events,
                ..current_engine_config.clone()
            };

            // SDK pre-builds an engine_config with handoff overrides
            // (model / session_id / cwd may differ from runtime
            // defaults). `build_engine_from_config` installs every
            // per-session subsystem via `wire_engine`, and the
            // `app_state_override` argument keeps the compaction
            // observers' app_state pointer aligned with the engine's —
            // critical so post-compact resets reach the actual flags
            // the engine reads, not a sibling runtime copy.
            // Seed the live permission base on the SDK session's app_state
            // (the engine below uses it via app_state_override) from this
            // turn's loaded rule maps, so the factory reads live rules + mode.
            // The rules + dirs live ONLY on the live base now. Preserve the
            // session working-dir allowlist already on the live base (seeded at
            // build from --add-dir + settings additionalDirectories, plus any
            // runtime /add-dir) so per-turn SDK rebuilds don't drop it (P17).
            {
                let mut guard = handoff.app_state.write().await;
                refresh_live_permissions_for_turn(
                    &mut guard,
                    SdkTurnPermissionInputs {
                        fallback_previous_mode: handoff.permission_mode.unwrap_or_default(),
                        permission_mode,
                        allow_rules,
                        deny_rules,
                        ask_rules,
                        permission_rule_source_roots,
                        plan_auto_options: coco_permissions::PlanModeAutoOptions {
                            use_auto_mode_during_plan: current_engine_config
                                .use_auto_mode_during_plan,
                            auto_mode_available: current_engine_config
                                .permission_mode_availability
                                .auto,
                        },
                    },
                );
            }

            let engine = runtime
                .build_engine_from_config(config, cancel, Some(handoff.app_state.clone()))
                .await
                .with_model_runtime_source(model_runtime_source);

            if let Some(request) = coco_commands::parse_goal_sentinel(&prompt) {
                let args = crate::goal_command::goal_display_args(&request).to_string();
                let gate = crate::goal_command::GoalGate {
                    hooks_restricted: current_engine_config.disable_all_hooks
                        || current_engine_config.allow_managed_hooks_only,
                    // SDK is non-interactive; the trust gate is deliberately skipped.
                    trust_rejected: false,
                };
                let tokens_at_start = runtime.session_usage_snapshot().await.totals.output_tokens;
                let history_snapshot = history_handle.lock().await.clone();
                let outcome = crate::goal_command::resolve_goal_request(
                    request,
                    &handoff.app_state,
                    &runtime.hook_registry(),
                    &history_snapshot,
                    tokens_at_start,
                    gate,
                )
                .await;

                match outcome {
                    crate::goal_command::GoalOutcome::Text(text) => {
                        sdk_append_slash_text(&history_handle, &event_tx, "goal", &args, &text)
                            .await;
                        return Ok(());
                    }
                    crate::goal_command::GoalOutcome::StatusThenText { status, text } => {
                        sdk_append_goal_status_and_slash_text(
                            &session,
                            &history_handle,
                            &event_tx,
                            status,
                            &args,
                            &text,
                        )
                        .await;
                        sdk_emit_active_goal_snapshot(&session, &handoff.app_state, &event_tx)
                            .await;
                        return Ok(());
                    }
                    crate::goal_command::GoalOutcome::SetAndRun {
                        status,
                        text,
                        kickoff,
                    } => {
                        sdk_append_goal_status(&history_handle, &event_tx, status).await;
                        sdk_emit_active_goal_snapshot(&session, &handoff.app_state, &event_tx)
                            .await;
                        sdk_append_slash_text(&history_handle, &event_tx, "goal", &args, &text)
                            .await;
                        prompt = kickoff;
                    }
                }
            }

            // Snapshot the prior history, append a fresh user message,
            // and **persist the combined history back to shared state
            // BEFORE calling the engine**. This way, even if the engine
            // returns `Err(...)` (e.g. transport crash, unrecoverable
            // tool failure), the user's prompt is still recorded and
            // the next `turn/start` sees it. On `Ok`, we overwrite with
            // the engine's more up-to-date `final_messages`, which also
            // includes any tool calls + the assistant reply.
            //
            // The engine's `run_session_loop` finds the LAST user
            // message in the list and keys the file history snapshot
            // against it, so passing the whole combined list works
            // for both single and multi-turn scenarios.
            // SDK-side `/compact` short-circuit. If the prompt arrives as
            // a sentinel-prefixed string (slash-command handler output),
            // run manual compaction directly rather than sending the
            // sentinel text to the LLM as a user message.
            if let Some(req) = coco_commands::handlers::compact::parse_compact_sentinel(&prompt) {
                let combined: Vec<std::sync::Arc<coco_messages::Message>> = {
                    let h = history_handle.lock().await;
                    h.clone()
                };
                let mut history = MessageHistory::new();
                for arc in combined {
                    history.push_arc(arc);
                }
                let command_args = req.custom_instructions;
                let custom_instructions = if command_args.is_empty() {
                    None
                } else {
                    Some(command_args.clone())
                };
                let event_tx_opt = Some(event_tx.clone());
                let request = coco_query::ManualCompactRequest {
                    custom_instructions,
                    command_args,
                };
                engine
                    .run_manual_compact(&mut history, &event_tx_opt, request)
                    .await;
                {
                    let mut h = history_handle.lock().await;
                    *h = history.to_vec();
                }
                return Ok(());
            }

            // SDK-side `/dream` short-circuit — fire auto-memory
            // consolidation directly. When the engine has no
            // `MemoryRuntime` (Feature::AutoMemory off), we silently
            // no-op. Uses `force` so the time / session / scan-throttle
            // gates are bypassed; the lock is still acquired.
            if coco_commands::handlers::dream::parse_dream_sentinel(&prompt).is_some() {
                if let Some(runtime) = engine.memory_runtime() {
                    let transcript_dir = runtime
                        .transcript_dir()
                        .map(std::path::Path::to_path_buf)
                        .unwrap_or_else(|| std::path::PathBuf::from("."));
                    let now_ms = coco_memory::service::dream::DreamService::now_ms();
                    let _ = runtime.dream.force(&transcript_dir, Vec::new, now_ms).await;
                }
                return Ok(());
            }

            // SDK-side `/rename [name]` short-circuit. The sentinel
            // arrives as the slash handler's first line; we resolve
            // the name (LLM-generated when `Auto`) and persist via
            // the shared helpers, then return without sending the
            // sentinel text to the LLM as a user message.
            if let Some(req) = coco_commands::parse_rename_sentinel(&prompt) {
                // Teammates can't rename — silently no-op for SDK
                // (no user-visible transcript) to mirror the TUI
                // guard without surfacing an error that wasn't
                // requested by an interactive user. Logged so the
                // call still leaves a trail.
                if coco_coordinator::identity::is_teammate() {
                    tracing::warn!("SDK rename ignored: session is a swarm teammate");
                    return Ok(());
                }

                let name = match req {
                    coco_commands::ParsedRename::Explicit(n) => n,
                    coco_commands::ParsedRename::Auto => {
                        match crate::session_rename::auto_generate_session_name(&session).await {
                            Ok(n) => n,
                            Err(err) => {
                                tracing::warn!(
                                    reason = ?err,
                                    "SDK rename auto-gen failed"
                                );
                                return Ok(());
                            }
                        }
                    }
                };
                if let Err(e) = crate::session_rename::persist_rename(&session, name.clone()).await
                {
                    tracing::warn!(
                        error = %e,
                        "SDK rename persist failed"
                    );
                }
                return Ok(());
            }

            // SDK-side `/summary` short-circuit — force a 9-section
            // session-memory update.
            if coco_commands::handlers::summary::parse_summary_sentinel(&prompt).is_some() {
                if let Some(runtime) = engine.memory_runtime() {
                    let combined: Vec<std::sync::Arc<coco_messages::Message>> = {
                        let h = history_handle.lock().await;
                        h.clone()
                    };
                    let tokens = coco_messages::estimate_tokens_for_messages(&combined);
                    // Walk history for the orphan-safe cursor signals.
                    let last_msg_id = combined
                        .last()
                        .and_then(|m| m.uuid())
                        .map(uuid::Uuid::to_string);
                    let had_tool_calls =
                        coco_messages::count_tool_calls_in_last_assistant_turn(&combined) > 0;
                    let _ = runtime
                        .session_memory
                        .force(tokens, last_msg_id, had_tool_calls)
                        .await;
                }
                return Ok(());
            }

            // SDK-side `/cost` short-circuit — render the live multi-provider
            // session cost snapshot instead of leaking the raw sentinel.
            if coco_commands::handlers::cost::parse_cost_sentinel(&prompt).is_some() {
                let snapshot = runtime.session_usage_snapshot().await;
                let text = coco_messages::format_session_cost(&snapshot);
                {
                    let mut h = history_handle.lock().await;
                    h.push(std::sync::Arc::new(coco_messages::create_meta_message(
                        &text,
                    )));
                }
                return Ok(());
            }

            // SDK-side `/status` short-circuit — render the live session status.
            if coco_commands::parse_status_sentinel(&prompt).is_some() {
                let text = runtime.status_report().await;
                {
                    let mut h = history_handle.lock().await;
                    h.push(std::sync::Arc::new(coco_messages::create_meta_message(
                        &text,
                    )));
                }
                return Ok(());
            }

            // SDK-side `/btw` short-circuit (D1). When the prompt is
            // the BTW sentinel emitted by `handlers::btw::handler`,
            // dispatch a one-shot fork via the runtime's
            // [`ForkDispatcher`] instead of mutating the parent
            // engine. The dispatcher builds a *fresh* engine, runs a
            // single turn against it, and returns the response text;
            // the parent's history and cache slot are untouched.
            if let Some(req) = coco_commands::handlers::btw::parse_btw_sentinel(&prompt) {
                // Shares the fork+extract logic with the TUI path
                // (`crate::side_question`): tool-less one-shot fork sharing the
                // parent cache, answer flattened across all per-block messages.
                let cache = match engine.last_cache_safe_params().await {
                    Some(cache) => cache,
                    None => {
                        let history = {
                            let h = history_handle.lock().await;
                            MessageHistory::from_arcs_preserving_latest_usage(h.clone())
                        };
                        engine.cache_safe_params_from_history(&history)
                    }
                };
                let response_text = match runtime.current_fork_dispatcher().await {
                    None => {
                        "(fork dispatcher not installed — /btw requires CLI bootstrap)".to_string()
                    }
                    Some(dispatcher) => {
                        crate::side_question::run_side_question_fork(
                            &cache,
                            &dispatcher,
                            &req.question,
                        )
                        .await
                    }
                };
                // Surface the answer through the same transcript-only
                // slash-command messages as the TUI path. A meta message
                // would be API-visible and would leak the side answer into the
                // parent model context on the next SDK turn.
                let messages = coco_messages::build_slash_command_messages(
                    "btw",
                    &req.question,
                    &response_text,
                    /*is_sensitive*/ false,
                );
                {
                    let mut h = history_handle.lock().await;
                    for msg in messages {
                        let msg = std::sync::Arc::new(msg);
                        h.push(msg.clone());
                        let _ = event_tx
                            .send(CoreEvent::Protocol(
                                coco_types::ServerNotification::MessageAppended {
                                    message: msg,
                                    identity: coco_types::ServerNotificationIdentity::default(),
                                },
                            ))
                            .await;
                    }
                }
                return Ok(());
            }

            // The handler minted and returned this id in the synchronous
            // `turn/start` response; lifecycle events must use the same id so
            // clients can correlate completion.
            let cycle_turn_id = turn_id;

            let combined: Vec<std::sync::Arc<coco_messages::Message>> = if history_override
                .is_empty()
            {
                // Fire UserPromptSubmit hooks BEFORE the LLM call. Output
                // surfaces as `hook_*` reminders on the next reminder pass;
                // a blocking_error suppresses the turn (warns instead);
                // prevent_continuation keeps the prompt but skips the
                // engine.
                let prompt_hook_result = runtime.fire_user_prompt_submit_hooks(&prompt).await;
                if let Some(blocking) = &prompt_hook_result.blocking_error {
                    let warning = format!(
                        "UserPromptSubmit hook blocked the turn: {}\n\nOriginal prompt: {prompt}",
                        blocking.blocking_error,
                    );
                    let warning_msg =
                        std::sync::Arc::new(coco_messages::create_user_message(&warning));
                    {
                        let mut h = history_handle.lock().await;
                        h.push(warning_msg.clone());
                    }
                    // I-1: emit so SDK observers see the warning row.
                    let _ = event_tx
                        .send(CoreEvent::Protocol(
                            coco_types::ServerNotification::MessageAppended {
                                message: warning_msg,
                                identity: coco_types::ServerNotificationIdentity::default(),
                            },
                        ))
                        .await;
                    // Pre-engine bail: emit a self-contained
                    // TurnStarted + TurnEnded(Failed) pair so SDK
                    // consumers see a complete cycle envelope. `HookBlocked`
                    // is the typed signal that this is a policy decision,
                    // not a runtime/config/provider error — lets dashboards
                    // filter "real failures" from "hook said no".
                    let _ = event_tx
                        .send(CoreEvent::Protocol(
                            coco_types::ServerNotification::TurnStarted(
                                coco_types::TurnStartedParams {
                                    turn_id: cycle_turn_id.clone(),
                                },
                            ),
                        ))
                        .await;
                    let _ = event_tx
                        .send(CoreEvent::Protocol(
                            coco_types::ServerNotification::TurnEnded(
                                coco_types::TurnEndedParams::failed(
                                    cycle_turn_id.clone(),
                                    /*usage*/ None,
                                    coco_types::ErrorPayload {
                                        message: warning.clone(),
                                        code: coco_types::ErrorCode::HookBlocked,
                                    },
                                ),
                            ),
                        ))
                        .await;
                    return Ok(());
                }
                if prompt_hook_result.prevent_continuation {
                    let stop_msg = prompt_hook_result
                        .stop_reason
                        .clone()
                        .map(|r| format!("Operation stopped by hook: {r}"))
                        .unwrap_or_else(|| "Operation stopped by hook".to_string());
                    let prompt_msg =
                        std::sync::Arc::new(coco_messages::create_user_message(&prompt));
                    let stop_msg_obj =
                        std::sync::Arc::new(coco_messages::create_user_message(&stop_msg));
                    {
                        let mut h = history_handle.lock().await;
                        h.push(prompt_msg.clone());
                        h.push(stop_msg_obj.clone());
                    }
                    // I-1: emit so SDK observers see both rows.
                    let _ = event_tx
                        .send(CoreEvent::Protocol(
                            coco_types::ServerNotification::MessageAppended {
                                message: prompt_msg,
                                identity: coco_types::ServerNotificationIdentity::default(),
                            },
                        ))
                        .await;
                    let _ = event_tx
                        .send(CoreEvent::Protocol(
                            coco_types::ServerNotification::MessageAppended {
                                message: stop_msg_obj,
                                identity: coco_types::ServerNotificationIdentity::default(),
                            },
                        ))
                        .await;
                    return Ok(());
                }

                // Resolve `@`-mentions in the prompt to file-content
                // system-reminder messages. A shared helper drives TUI /
                // headless / SDK identically — without this, headless and
                // SDK clients sending `@path/to/file` got the literal string
                // instead of the file's contents (the `at_mentioned_files`
                // reminder body claims content is "loaded into context" —
                // this is what makes that true).
                let cwd_path = std::path::Path::new(&handoff.cwd);
                let inputs = crate::at_mention_turn::resolve_turn_inputs(
                    &prompt,
                    &images,
                    cwd_path,
                    uuid::Uuid::new_v4(),
                    &runtime.file_read_state,
                )
                .await;
                let mut new_msgs = Vec::new();
                if let Some(metadata) = slash_metadata.as_deref() {
                    new_msgs.push(create_slash_metadata_message(metadata));
                }
                new_msgs.extend(crate::at_mention_turn::build_messages_for_turn(&inputs));
                // I-1 (Authority) — D2: emit MessageAppended for the new
                // turn messages BEFORE invoking the engine. The engine no
                // longer re-emits its initial turn_messages load (would
                // double-fire on every turn). Engines only emit for
                // newly-produced content (assistant turns, tool results,
                // system pushes) within the loop. See
                // `engine-tui-unified-transcript-plan.md` §5.2.
                for m in new_msgs.iter().cloned() {
                    let _ = event_tx
                        .send(CoreEvent::Protocol(
                            coco_types::ServerNotification::MessageAppended {
                                message: std::sync::Arc::new(m),
                                identity: coco_types::ServerNotificationIdentity::default(),
                            },
                        ))
                        .await;
                }
                let new_msg_arcs: Vec<std::sync::Arc<coco_messages::Message>> =
                    new_msgs.into_iter().map(std::sync::Arc::new).collect();
                let combined: Vec<std::sync::Arc<coco_messages::Message>> = {
                    let mut h = history_handle.lock().await;
                    h.extend(new_msg_arcs.iter().cloned());
                    h.clone()
                };
                {
                    let mut h = runtime.history.lock().await;
                    for msg in new_msg_arcs {
                        h.push_arc(msg);
                    }
                }
                if !inputs.mentioned_paths.is_empty() {
                    engine
                        .note_mentioned_paths(inputs.mentioned_paths.clone())
                        .await;
                }
                combined
            } else {
                let override_messages: Vec<std::sync::Arc<coco_messages::Message>> =
                    history_override
                        .into_iter()
                        .map(serde_json::from_value::<coco_messages::Message>)
                        .collect::<Result<Vec<_>, _>>()?
                        .into_iter()
                        .map(std::sync::Arc::new)
                        .collect();
                {
                    let mut h = history_handle.lock().await;
                    *h = override_messages.clone();
                }
                {
                    let mut h = runtime.history.lock().await;
                    h.clear();
                    for msg in override_messages.iter().cloned() {
                        h.push_arc(msg);
                    }
                }
                override_messages
            };

            // Clone the event channel so we can still emit on the
            // error path (the engine takes ownership of the original).
            let event_tx_for_error = event_tx.clone();
            let session_id_for_error = handoff.session_id.clone();
            let (core_event_tx, mut core_event_rx) = mpsc::channel::<CoreEvent>(256);
            let event_tx_forward = event_tx.clone();
            let session_manager_for_forward = Arc::clone(&runtime.session_manager);
            let session_id_for_forward = handoff.session_id.clone();
            let forward_handle = tokio::spawn(async move {
                while let Some(event) = core_event_rx.recv().await {
                    if matches!(
                        event,
                        CoreEvent::Protocol(coco_types::ServerNotification::ContextCompacted(_))
                    ) {
                        let manager = Arc::clone(&session_manager_for_forward);
                        let session_id = session_id_for_forward.clone();
                        let _ = tokio::task::spawn_blocking(move || {
                            let _ = manager.re_append_session_metadata(session_id.as_str());
                        })
                        .await;
                    }
                    if event_tx_forward.send(event).await.is_err() {
                        break;
                    }
                }
            });

            let engine_result = engine
                .run_with_messages(combined, core_event_tx, cycle_turn_id.clone())
                .await;
            let _ = forward_handle.await;

            match engine_result {
                Ok(result) => {
                    info!(
                        turns = result.turns,
                        input_tokens = result.total_usage.input_tokens.total,
                        output_tokens = result.total_usage.output_tokens.total,
                        history_len = result.final_messages.len(),
                        "QueryEngineRunner: turn complete"
                    );
                    // Overwrite with the engine's final history — this
                    // includes tool calls, tool results, and the
                    // assistant reply in addition to the user message
                    // we pre-persisted above.
                    let final_messages = result.final_messages.clone();
                    let final_history = result.final_history.snapshot();
                    {
                        let mut h = history_handle.lock().await;
                        *h = final_messages;
                    }
                    {
                        let mut h = runtime.history.lock().await;
                        *h = final_history;
                    }
                    // Sole Interrupted emit site. Fires when either the
                    // engine observed cancel mid-loop (`result.cancelled`
                    // = true → engine returned Ok with cancelled marker)
                    // OR the cancel raced and arrived after Ok return
                    // (`cancel_for_terminal.is_cancelled()`). The engine
                    // no longer wire-emits Interrupted — runner owns the
                    // single terminator. Reason is hardcoded
                    // `UserCancel`: SDK has only the `turn/interrupt`
                    // control message as a cancel source, which is by
                    // definition user-initiated. (TUI has the broader
                    // UserCancel-vs-SystemPreempt split because of
                    // `/clear` / `/compact` / `/rewind` — SDK has no
                    // equivalent runner-level cancel arms.)
                    if result.cancelled || cancel_for_terminal.is_cancelled() {
                        let reason = match result.stop_reason.as_deref() {
                            Some("permission_abort") => {
                                coco_types::TurnAbortReason::PermissionAbort
                            }
                            _ => coco_types::TurnAbortReason::UserCancel,
                        };
                        let _ = event_tx_for_error
                            .send(CoreEvent::Protocol(
                                coco_types::ServerNotification::TurnEnded(
                                    coco_types::TurnEndedParams::interrupted(
                                        cycle_turn_id.clone(),
                                        /*usage*/ None,
                                        reason,
                                    ),
                                ),
                            ))
                            .await;
                    }
                    Ok(())
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        "QueryEngineRunner: engine returned error; \
                         user message already persisted to session history"
                    );
                    // Engine-bail path: when cancel was the cause the
                    // engine_session Err branch skipped its `Failed`
                    // emit, so we synthesize the Interrupted terminator
                    // here. When it's a true error the engine_session
                    // already emitted `Failed` — no second terminator
                    // needed.
                    if cancel_for_terminal.is_cancelled() {
                        let _ = event_tx_for_error
                            .send(CoreEvent::Protocol(
                                coco_types::ServerNotification::TurnEnded(
                                    coco_types::TurnEndedParams::interrupted(
                                        cycle_turn_id.clone(),
                                        /*usage*/ None,
                                        coco_types::TurnAbortReason::UserCancel,
                                    ),
                                ),
                            ))
                            .await;
                    }

                    // Emit a synthetic `SessionResult` with `is_error=true`
                    // so the forwarder's `accumulate_session_result` folds
                    // the failure into the SDK session stats accumulator. Without
                    // this, true engine-bail paths (compaction failure,
                    // transport crash, etc.) don't surface in the final
                    // aggregated `SessionResult` emitted by `session/archive`.
                    //
                    // Fields are minimal — we don't have usage/cost
                    // because the engine didn't reach `make_result`. The
                    // forwarder handles missing fields gracefully (default
                    // usage is zero; cost is 0.0; errors list is the one
                    // message we provide).
                    let error_params = coco_types::SessionResultParams {
                        session_id: session_id_for_error.clone(),
                        total_turns: 1,
                        duration_ms: 0,
                        duration_api_ms: 0,
                        is_error: true,
                        stop_reason: if cancel_for_terminal.is_cancelled() {
                            "interrupted".into()
                        } else {
                            "engine_error".into()
                        },
                        total_cost_usd: 0.0,
                        usage: coco_types::TokenUsage::default(),
                        model_usage: std::collections::HashMap::new(),
                        permission_denials: Vec::new(),
                        result: None,
                        errors: vec![e.to_string()],
                        structured_output: None,
                        fast_mode_state: None,
                        num_api_calls: None,
                    };
                    let _ = event_tx_for_error
                        .send(CoreEvent::Protocol(
                            coco_types::ServerNotification::SessionResult(Box::new(error_params)),
                        ))
                        .await;
                    Err(anyhow::anyhow!("{e}"))
                }
            }
        })
    }
}

struct SdkTurnPermissionInputs {
    fallback_previous_mode: coco_types::PermissionMode,
    permission_mode: coco_types::PermissionMode,
    allow_rules: coco_types::PermissionRulesBySource,
    deny_rules: coco_types::PermissionRulesBySource,
    ask_rules: coco_types::PermissionRulesBySource,
    permission_rule_source_roots:
        std::collections::HashMap<coco_types::PermissionRuleSource, std::path::PathBuf>,
    plan_auto_options: coco_permissions::PlanModeAutoOptions,
}

fn refresh_live_permissions_for_turn(
    guard: &mut coco_types::ToolAppState,
    refresh: SdkTurnPermissionInputs,
) {
    let previous_mode = guard
        .permissions
        .mode
        .unwrap_or(refresh.fallback_previous_mode);
    guard.permissions.allow_rules = refresh.allow_rules;
    guard.permissions.deny_rules = refresh.deny_rules;
    guard.permissions.ask_rules = refresh.ask_rules;
    guard.permissions.permission_rule_source_roots = refresh.permission_rule_source_roots;
    let live_allow_rules = guard.permissions.allow_rules.clone();
    coco_permissions::apply_permission_mode_transition_to_app_state(
        guard,
        previous_mode,
        refresh.permission_mode,
        &live_allow_rules,
        refresh.plan_auto_options,
    );
}

async fn sdk_append_slash_text(
    history_handle: &Arc<tokio::sync::Mutex<Vec<Arc<coco_messages::Message>>>>,
    event_tx: &mpsc::Sender<CoreEvent>,
    command: &str,
    args: &str,
    text: &str,
) {
    let messages = coco_messages::build_slash_command_messages(
        command, args, text, /*is_sensitive*/ false,
    );
    sdk_append_messages(history_handle, event_tx, messages).await;
}

async fn sdk_append_goal_status(
    history_handle: &Arc<tokio::sync::Mutex<Vec<Arc<coco_messages::Message>>>>,
    event_tx: &mpsc::Sender<CoreEvent>,
    payload: coco_types::GoalStatusPayload,
) {
    sdk_append_messages(
        history_handle,
        event_tx,
        vec![coco_messages::Message::Attachment(
            coco_messages::AttachmentMessage::silent_goal_status(payload),
        )],
    )
    .await;
}

async fn sdk_append_goal_status_and_slash_text(
    session: &crate::session_runtime::SessionHandle,
    history_handle: &Arc<tokio::sync::Mutex<Vec<Arc<coco_messages::Message>>>>,
    event_tx: &mpsc::Sender<CoreEvent>,
    payload: coco_types::GoalStatusPayload,
    args: &str,
    text: &str,
) {
    let mut messages = vec![coco_messages::Message::Attachment(
        coco_messages::AttachmentMessage::silent_goal_status(payload),
    )];
    messages.extend(coco_messages::build_slash_command_messages(
        "goal", args, text, /*is_sensitive*/ false,
    ));
    sdk_append_messages(history_handle, event_tx, messages.clone()).await;
    let runtime = session.runtime();
    runtime.persist_local_transcript_messages(&messages).await;
}

async fn sdk_emit_active_goal_snapshot(
    session: &crate::session_runtime::SessionHandle,
    app_state: &tokio::sync::RwLock<coco_types::ToolAppState>,
    event_tx: &mpsc::Sender<CoreEvent>,
) {
    let goal = app_state.read().await.active_goal.clone();
    let _ = event_tx
        .send(CoreEvent::Protocol(
            crate::goal_command::active_goal_changed_notification(goal.clone()),
        ))
        .await;
    let runtime = session.runtime();
    runtime
        .persist_goal_metadata(goal.as_ref().map(|goal| {
            coco_session::GoalMetadata::from_active_goal(goal, /*met*/ false)
        }))
        .await;
}

async fn sdk_append_messages(
    history_handle: &Arc<tokio::sync::Mutex<Vec<Arc<coco_messages::Message>>>>,
    event_tx: &mpsc::Sender<CoreEvent>,
    messages: Vec<coco_messages::Message>,
) {
    let mut history = history_handle.lock().await;
    for message in messages {
        let message = Arc::new(message);
        history.push(message.clone());
        let _ = event_tx
            .send(CoreEvent::Protocol(
                coco_types::ServerNotification::MessageAppended {
                    message,
                    identity: coco_types::ServerNotificationIdentity::default(),
                },
            ))
            .await;
    }
}

#[cfg(test)]
#[path = "sdk_runner.test.rs"]
mod tests;
