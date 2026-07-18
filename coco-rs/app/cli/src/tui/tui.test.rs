//! Unit tests for the TUI driver's pure helpers.
//!
//! `run_agent_driver` itself is an integration point (talks to model
//! runtimes, spawns tokio tasks, etc.) so we exercise only the
//! decomposed pure logic here.

#[test]
fn latest_todo_write_todos_reads_last_tool_call_input() {
    let older = coco_messages::create_assistant_message(
        vec![coco_messages::AssistantContent::tool_call(
            "toolu_1",
            coco_types::ToolName::TodoWrite.as_str(),
            serde_json::json!({
                "todos": [
                    {
                        "content": "old item",
                        "status": "pending",
                        "activeForm": "Doing old item"
                    }
                ]
            }),
        )],
        "test-model",
        Default::default(),
    );
    let newer = coco_messages::create_assistant_message(
        vec![coco_messages::AssistantContent::tool_call(
            "toolu_2",
            coco_types::ToolName::TodoWrite.as_str(),
            serde_json::json!({
                "todos": [
                    {
                        "content": "new item",
                        "status": "in_progress",
                        "activeForm": "Doing new item"
                    }
                ]
            }),
        )],
        "test-model",
        Default::default(),
    );

    let todos = super::latest_todo_write_todos(&[older, newer]).expect("todos restored");
    assert_eq!(todos.len(), 1);
    assert_eq!(todos[0].content, "new item");
    assert_eq!(todos[0].status, "in_progress");
    assert_eq!(todos[0].active_form, "Doing new item");
}

#[test]
fn latest_todo_write_todos_clears_all_completed_snapshot() {
    let message = coco_messages::create_assistant_message(
        vec![coco_messages::AssistantContent::tool_call(
            "toolu_1",
            coco_types::ToolName::TodoWrite.as_str(),
            serde_json::json!({
                "todos": [
                    {
                        "content": "done item",
                        "status": "completed",
                        "activeForm": "Doing done item"
                    }
                ]
            }),
        )],
        "test-model",
        Default::default(),
    );

    let todos = super::latest_todo_write_todos(&[message]).expect("todos restored");
    assert!(todos.is_empty());
}

#[cfg(test)]
mod plan_prompt_editor_tests {
    use super::super::PendingEditorRequest;
    use super::super::emit_editor_prepare_failed;

    #[tokio::test]
    async fn prepare_failure_for_plan_prompt_returns_prompt_local_event() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);

        emit_editor_prepare_failed(
            PendingEditorRequest::PlanPrompt {
                request_id: "perm-1".to_string(),
                initial_content: "# Plan".to_string(),
                path: None,
            },
            "raw mode failed".to_string(),
            tx,
        )
        .await;

        let event = rx.try_recv().expect("failure event sent");
        let coco_types::CoreEvent::Tui(coco_types::TuiOnlyEvent::ExitPlanPromptEditorFailed {
            request_id,
            error,
        }) = event
        else {
            panic!("expected ExitPlanPromptEditorFailed, got {event:?}")
        };
        assert_eq!(request_id, "perm-1");
        assert!(error.contains("raw mode failed"));
    }
}

#[cfg(test)]
mod goal_tests {
    use super::super::workspace_trust_rejected_from_env;

    #[test]
    fn workspace_trust_gate_only_rejects_explicit_zero() {
        assert!(workspace_trust_rejected_from_env(Some("0")));
        assert!(!workspace_trust_rejected_from_env(Some("1")));
        assert!(!workspace_trust_rejected_from_env(None));
    }
}

use super::ActiveTurn;
use super::ActiveTurnCancel;
use super::ActiveTurnDrain;
use super::LocalRuntimeControlContext;
use super::SentinelTrigger;
use super::apply_resume_plan_through_app_server;
use super::background_all_tasks_through_app_server;
use super::classify_sentinel_trigger;
use super::dispatch_slash_command;
use super::drain_active_turn;
use super::drain_completed_turn;
use super::handle_rewind;
use super::parse_editor_command;
use super::parse_slash_command;
use super::process_idle_command_queue;
use super::run_clear_conversation;
use super::run_dream_consolidation;
use super::run_manual_compact;
use super::run_prompt_mode_bash;
use super::run_session_memory_force;
use super::run_session_rename;
use super::run_session_tag;
use super::run_show_cost;
use super::run_show_status;
use super::run_side_chat;
use super::session_plan_file_path;
use super::set_thinking_level_through_app_server;
use super::should_trigger_title_gen;
use super::slash_unavailable_in_session_message;
use super::toggle_fast_mode_through_app_server;
use clap::Parser;
use coco_agent_host::session_controls::PermissionsMutation;
use coco_agent_host::session_controls::directory_already_accessible_message;
use coco_agent_host::session_controls::parse_permissions_mutation;
use coco_config::CatalogPaths;
use coco_config::EnvSnapshot;
use coco_config::RoleSlots;
use coco_config::RuntimeOverrides;
use coco_config::Settings;
use coco_config::SettingsWithSource;
use coco_tui::SystemPushKind;
use coco_types::CommandArgumentKind;
use coco_types::CommandBase;
use coco_types::CommandSafety;
use coco_types::CommandSource;
use coco_types::CommandType;
use coco_types::LocalCommandData;
use coco_types::ModelRole;
use coco_types::ProviderModelSelection;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::Mutex;

fn expect_session_scoped_protocol(
    event: coco_types::CoreEvent,
) -> (coco_types::SessionId, coco_types::ServerNotification) {
    match event {
        coco_types::CoreEvent::Tui(coco_types::TuiOnlyEvent::SessionScoped {
            session_id,
            event,
        }) => match *event {
            coco_types::SessionScopedEvent::Protocol(notification) => (session_id, *notification),
            other => panic!("expected protocol event in session scope, got {other:?}"),
        },
        other => panic!("expected session-scoped protocol event, got {other:?}"),
    }
}

#[test]
fn directory_already_accessible_message_distinguishes_current_added_and_nested() {
    let current = std::path::Path::new("/repo");
    let additional = vec![std::path::PathBuf::from("/opt/shared")];

    assert_eq!(
        directory_already_accessible_message(std::path::Path::new("/repo"), current, &additional)
            .as_deref(),
        Some("/repo is already the current working directory.")
    );
    assert_eq!(
        directory_already_accessible_message(
            std::path::Path::new("/opt/shared"),
            current,
            &additional,
        )
        .as_deref(),
        Some("/opt/shared is already added as a working directory.")
    );
    assert_eq!(
        directory_already_accessible_message(
            std::path::Path::new("/repo/src"),
            current,
            &additional,
        )
        .as_deref(),
        Some("/repo/src is already accessible within the current working directory /repo.")
    );
    assert_eq!(
        directory_already_accessible_message(
            std::path::Path::new("/opt/shared/tools"),
            current,
            &additional,
        )
        .as_deref(),
        Some(
            "/opt/shared/tools is already accessible within the additional working directory /opt/shared."
        )
    );
    assert!(
        directory_already_accessible_message(
            std::path::Path::new("/tmp/other"),
            current,
            &additional,
        )
        .is_none()
    );
}

#[test]
fn title_gen_fires_when_all_conditions_met() {
    assert!(should_trigger_title_gen(
        /*auto_title_enabled*/ true, /*already_attempted*/ false,
        /*fast_spec_present*/ true, /*plan_has_exited*/ true,
        /*plan_text_non_empty*/ true,
    ));
}

#[tokio::test]
async fn shutdown_drain_aborts_stuck_active_turn_after_timeout() {
    struct DropFlag(Arc<AtomicBool>);

    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    let dropped = Arc::new(AtomicBool::new(false));
    let dropped_for_task = dropped.clone();
    let task = tokio::spawn(async move {
        let _guard = DropFlag(dropped_for_task);
        std::future::pending::<()>().await;
    });
    let bridge = coco_agent_host::app_server_host::AppServerLocalBridge::new(Arc::new(
        coco_agent_host::app_server_host::AppServerHostState::default(),
    ));
    let slot = Arc::new(Mutex::new(Some(ActiveTurn {
        id: uuid::Uuid::new_v4(),
        task,
        cancel: ActiveTurnCancel {
            client: bridge.connect_local_client(),
            handler: bridge.handler().clone(),
            target: coco_types::InteractiveTarget {
                session_id: coco_types::SessionId::generate(),
                surface_id: coco_types::SurfaceId::generate(),
            },
        },
    })));

    drain_active_turn(
        &slot,
        ActiveTurnDrain::AbortAfter(Duration::from_millis(10)),
    )
    .await;

    assert!(slot.lock().await.is_none());
    assert!(dropped.load(Ordering::SeqCst));
}

#[test]
fn title_gen_gated_off_by_setting() {
    // User hasn't opted in.
    assert!(!should_trigger_title_gen(false, false, true, true, true));
}

#[test]
fn title_gen_does_not_retry_after_first_attempt() {
    // Latch: once we've attempted, don't re-fire even if conditions still hold.
    assert!(!should_trigger_title_gen(true, true, true, true, true));
}

#[test]
fn title_gen_skipped_without_fast_model() {
    // User enabled auto_title but hasn't wired up a Fast role / the
    // `ANTHROPIC_API_KEY` fallback isn't available. Silent skip.
    assert!(!should_trigger_title_gen(true, false, false, true, true));
}

#[test]
fn title_gen_skipped_before_plan_exited() {
    // Model hasn't successfully exited plan mode yet this session.
    assert!(!should_trigger_title_gen(true, false, true, false, true));
}

#[test]
fn title_gen_skipped_with_empty_plan() {
    // ExitPlanMode ran against an empty plan file (e.g. model called
    // Exit before writing anything). No useful context to summarize.
    assert!(!should_trigger_title_gen(true, false, true, true, false));
}

#[test]
fn remote_model_change_reminder_is_remote_main_only() {
    let msg = coco_agent_host::session_messages::remote_model_change_reminder_message(
        ModelRole::Main,
        "Claude Sonnet 4.6",
        true,
    )
    .expect("remote main switch should emit reminder");
    let text = coco_messages::wrapping::extract_text_from_message(&msg);
    assert!(text.contains("<system-reminder>"));
    assert!(text.contains("The model for this session has been changed to Claude Sonnet 4.6."));
    assert!(text.contains("You are now running as Claude Sonnet 4.6."));

    assert!(
        coco_agent_host::session_messages::remote_model_change_reminder_message(
            ModelRole::Main,
            "Claude Sonnet 4.6",
            false,
        )
        .is_none()
    );
    assert!(
        coco_agent_host::session_messages::remote_model_change_reminder_message(
            ModelRole::Plan,
            "Claude Sonnet 4.6",
            true,
        )
        .is_none()
    );
}

#[test]
fn permission_retry_system_push_builds_permission_retry_message() {
    let msg = crate::tui::driver::tui_system_push_message(SystemPushKind::PermissionRetry {
        tool_name: "Bash".into(),
        message: "Permission granted for: Bash. You may now retry this command if you would like."
            .into(),
    });

    match msg {
        coco_messages::Message::System(coco_messages::SystemMessage::PermissionRetry(retry)) => {
            assert_eq!(retry.tool_name, "Bash");
            assert_eq!(
                retry.message,
                "Permission granted for: Bash. You may now retry this command if you would like."
            );
        }
        other => panic!("expected PermissionRetry system message, got {other:?}"),
    }
}

#[test]
fn parse_slash_extracts_name_only() {
    assert_eq!(parse_slash_command("/help"), Some(("help", "")));
}

#[test]
fn parse_slash_splits_args() {
    assert_eq!(
        parse_slash_command("/commit focus on auth changes"),
        Some(("commit", "focus on auth changes"))
    );
}

#[test]
fn parse_slash_collapses_extra_whitespace() {
    // Single space after the name is the conventional separator;
    // additional whitespace is preserved as part of args (the
    // handlers themselves trim again).
    assert_eq!(
        parse_slash_command("/commit   spaced"),
        Some(("commit", "spaced"))
    );
}

#[test]
fn parse_slash_trims_outer_whitespace() {
    assert_eq!(parse_slash_command("   /diff   "), Some(("diff", "")));
}

#[test]
fn parse_slash_rejects_non_slash() {
    assert_eq!(parse_slash_command("hello world"), None);
}

#[test]
fn parse_slash_rejects_bare_slash() {
    assert_eq!(parse_slash_command("/"), None);
    assert_eq!(parse_slash_command("   /   "), None);
}

fn inactive_test_command_enabled() -> bool {
    false
}

fn inactive_test_command_handler(_args: &str) -> String {
    "handler should not run".to_string()
}

struct QueuedTurnMockModel;

#[async_trait::async_trait]
impl coco_inference::LanguageModel for QueuedTurnMockModel {
    fn provider(&self) -> &str {
        "mock"
    }

    fn model_id(&self) -> &str {
        "queued-turn-mock"
    }

    async fn do_generate(
        &self,
        _options: &coco_inference::LanguageModelCallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<coco_inference::LanguageModelGenerateResult, coco_inference::AISdkError> {
        Ok(coco_inference::LanguageModelGenerateResult {
            content: vec![coco_llm_types::AssistantContentPart::Text(
                coco_llm_types::TextPart {
                    text: "queued turn complete".into(),
                    provider_metadata: None,
                },
            )],
            usage: coco_llm_types::Usage::new(1, 1),
            finish_reason: coco_llm_types::FinishReason::new(coco_llm_types::StopReason::EndTurn),
            warnings: Vec::new(),
            provider_metadata: None,
            request: None,
            response: None,
        })
    }

    async fn do_stream(
        &self,
        options: &coco_inference::LanguageModelCallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<coco_inference::LanguageModelStreamResult, coco_inference::AISdkError> {
        let result = self.do_generate(options, None).await?;
        Ok(coco_inference::synthetic_stream_from_content(
            result.content,
            result.usage,
            result.finish_reason,
        ))
    }
}

async fn build_runtime_with_registry(
    home: &TempDir,
    registry: coco_commands::CommandRegistry,
) -> crate::session_runtime::SessionHandle {
    build_runtime_with_registry_and_settings(home, registry, Settings::default()).await
}

async fn build_runtime_with_registry_and_settings(
    home: &TempDir,
    registry: coco_commands::CommandRegistry,
    settings_overrides: Settings,
) -> crate::session_runtime::SessionHandle {
    build_runtime_with_profile(
        home,
        registry,
        settings_overrides,
        crate::session_runtime::SessionExecutionProfile::Primary,
    )
    .await
}

async fn build_runtime_with_profile(
    home: &TempDir,
    registry: coco_commands::CommandRegistry,
    settings_overrides: Settings,
    execution_profile: crate::session_runtime::SessionExecutionProfile,
) -> crate::session_runtime::SessionHandle {
    let session_backend = settings_overrides.session.backend;
    let settings = SettingsWithSource {
        merged: Settings {
            models: coco_config::ModelSelectionSettings {
                main: Some(RoleSlots::new(ProviderModelSelection {
                    provider: "anthropic".into(),
                    model_id: "claude-opus-4-7".into(),
                })),
                ..Default::default()
            },
            available_models: settings_overrides.available_models,
            file_checkpointing_enabled: settings_overrides.file_checkpointing_enabled,
            respond_to_bash_commands: settings_overrides.respond_to_bash_commands,
            session: settings_overrides.session,
            ..Default::default()
        },
        per_source: std::collections::HashMap::new(),
        source_paths: std::collections::HashMap::new(),
    };
    let runtime_config = coco_config::build_runtime_config_with(
        settings,
        EnvSnapshot::default(),
        RuntimeOverrides::default(),
        CatalogPaths::empty_in(home.path()),
        coco_config::parse_enabled_setting_sources(None),
    )
    .expect("runtime config");
    let model_id = coco_agent_host::headless::resolve_main_model(&runtime_config).model_id;
    let cli = coco_cli::Cli::try_parse_from(["coco"])
        .expect("parse cli")
        .agent_host_options();

    crate::session_runtime::SessionHandle::build(crate::session_runtime::SessionRuntimeBuildOpts {
        cli: &cli,
        runtime_config: Arc::new(runtime_config),
        config_reloader: None,
        cwd: home.path().to_path_buf(),
        model_id,
        system_prompt: "test".to_string(),
        permission_mode_availability: coco_types::PermissionModeAvailability::default(),
        permission_mode: coco_types::PermissionMode::default(),
        model_runtimes: Some(coco_query::test_support::model_runtime_registry(Arc::new(
            QueuedTurnMockModel,
        ))),
        tools: Arc::new(coco_tool_runtime::ToolRegistry::new()),
        session_manager: Arc::new(coco_session::SessionManager::with_backend(
            session_backend,
            home.path().join("sessions"),
        )),
        fast_model_spec: None,
        permission_bridge: None,
        command_registry: Arc::new(tokio::sync::RwLock::new(Arc::new(registry))),
        skill_manager: Arc::new(coco_skills::SkillManager::new()),
        process_runtime: coco_app_runtime::ProcessRuntime::global(),
        project_services: Arc::new(coco_app_runtime::ProjectServices::load(
            home.path(),
            home.path(),
        )),
        agent_search_paths: coco_subagent::definition_store::AgentSearchPaths::empty(),
        builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog::interactive(),
        session_id_override: None,
        is_non_interactive: false,
        execution_profile,
        callback_requirements: Default::default(),
    })
    .await
    .expect("build runtime")
}

async fn test_resume_context(
    runtime: &crate::session_runtime::SessionHandle,
) -> (
    super::SharedSessionHandle,
    crate::session_runtime::SessionRuntimeFactory,
    Arc<coco_app_runtime::ProcessRuntime>,
    std::path::PathBuf,
) {
    let rt = &runtime;
    let config = rt.current_engine_config().await;
    let cli = coco_cli::Cli::try_parse_from(["coco"])
        .expect("parse cli")
        .agent_host_options();
    let process_runtime = Arc::clone(rt.process_runtime());
    let cwd = rt.original_cwd().clone();
    let factory = crate::session_runtime::SessionRuntimeFactory::new(
        crate::session_runtime::SessionRuntimeFactoryOpts {
            cli: Arc::new(cli),
            bootstrap_source:
                crate::session_runtime::SessionRuntimeBootstrapSource::from_prebuilt_bootstrap(
                    crate::session_runtime::SessionRuntimeBootstrap {
                        runtime_config: Arc::clone(rt.runtime_config()),
                        tools: Arc::new(coco_tool_runtime::ToolRegistry::new()),
                        model_id: config.model_id,
                        system_prompt: config.system_prompt.unwrap_or_default(),
                        permission_mode_availability: config.permission_mode_availability,
                        permission_mode: config.permission_mode,
                        command_registry: Arc::new(tokio::sync::RwLock::new(Arc::new(
                            coco_commands::CommandRegistry::new(),
                        ))),
                        skill_manager: Arc::new(coco_skills::SkillManager::new()),
                        project_services: Arc::clone(rt.project_services()),
                        agent_search_paths:
                            coco_subagent::definition_store::AgentSearchPaths::empty(),
                    },
                ),
            cwd: cwd.clone(),
            model_runtimes: Some(coco_query::test_support::model_runtime_registry(Arc::new(
                QueuedTurnMockModel,
            ))),
            session_manager: rt.session_manager_handle(),
            fast_model_spec: rt.fast_model_spec().cloned(),
            permission_bridge: None,
            process_runtime: Arc::clone(&process_runtime),
            builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog::interactive(),
            is_non_interactive: false,
        },
    );
    (
        Arc::new(tokio::sync::RwLock::new(runtime.clone())),
        factory,
        process_runtime,
        cwd,
    )
}

fn local_bridge_for_resume_test(
    runtime: &crate::session_runtime::SessionHandle,
    runtime_factory: crate::session_runtime::SessionRuntimeFactory,
    process_runtime: Arc<coco_app_runtime::ProcessRuntime>,
    event_sink: Option<tokio::sync::mpsc::Sender<coco_types::CoreEvent>>,
) -> coco_agent_host::app_server_host::AppServerLocalBridge {
    coco_agent_host::app_server_host::AppServerLocalBridge::with_host_inputs_and_server_config(
        coco_agent_host::app_server_host::HostInputs {
            startup_cwd: Some(runtime.original_cwd().clone()),
            session_manager: Some(runtime.session_manager_handle()),
            runtime_replacement: Some(
                coco_agent_host::app_server_host::RuntimeReplacementContext {
                    runtime_factory,
                    process_runtime,
                    cwd: runtime.original_cwd().clone(),
                    requires_structured_output: false,
                    integration_options:
                        coco_agent_host::session_bootstrap::SessionIntegrationOptions {
                            event_sink,
                            ..Default::default()
                        },
                },
            ),
            turn_runner: Some(Arc::new(
                coco_agent_host::app_server_host::SessionTurnExecutor::new(None, None),
            )),
            ..Default::default()
        },
        &runtime.runtime_config().server,
    )
}

async fn install_test_session_runtime(
    bridge: &mut coco_agent_host::app_server_host::AppServerLocalBridge,
    runtime: &crate::session_runtime::SessionHandle,
) {
    bridge
        .bind_interactive_session(runtime.clone(), None)
        .await
        .expect("test runtime should bind to an interactive AppServer surface");
}

async fn dispatch_slash_command_for_test(
    name: &str,
    args: &str,
    runtime: &crate::session_runtime::SessionHandle,
    event_tx: &tokio::sync::mpsc::Sender<coco_types::CoreEvent>,
    local_app_server_bridge: &mut coco_agent_host::app_server_host::AppServerLocalBridge,
) -> super::SlashOutcome {
    let (current_session, _runtime_factory, _process_runtime, _cwd) =
        test_resume_context(runtime).await;
    let reload_subscriptions = test_runtime_reload_subscriptions(&current_session, event_tx);
    super::dispatch_slash_command(
        name,
        args,
        runtime,
        &current_session,
        event_tx,
        local_app_server_bridge,
        &reload_subscriptions,
        &[],
    )
    .await
}

fn test_runtime_reload_subscriptions(
    current_session: &super::SharedSessionHandle,
    event_tx: &tokio::sync::mpsc::Sender<coco_types::CoreEvent>,
) -> Arc<Mutex<super::TuiRuntimeReloadSubscriptions>> {
    let (subscriptions, _display_rx, _error_rx) = super::TuiRuntimeReloadSubscriptions::new(
        Arc::clone(current_session),
        event_tx.clone(),
        coco_agent_host::tui_permission_bridge::new_pending_map(),
    );
    Arc::new(Mutex::new(subscriptions))
}

async fn seed_runtime_session_transcript(runtime: &crate::session_runtime::SessionHandle) {
    let rt = &runtime;
    let session_id = rt.current_typed_session_id().await;
    let cwd = rt.original_cwd().clone();
    seed_session_transcript_for_cwd(rt.session_manager_handle().memory_base(), &cwd, &session_id);
}

fn seed_session_transcript_for_cwd(
    memory_base: &std::path::Path,
    cwd: &std::path::Path,
    session_id: &coco_types::SessionId,
) {
    let store = coco_session::TranscriptStore::new(std::sync::Arc::new(
        coco_paths::ProjectPaths::new(memory_base.to_path_buf(), cwd),
    ));
    append_seed_transcript(&store, cwd, session_id);
}

fn append_seed_transcript(
    store: &coco_session::TranscriptStore,
    cwd: &std::path::Path,
    session_id: &coco_types::SessionId,
) {
    let entry = coco_session::TranscriptEntry {
        entry_type: "user".to_string(),
        uuid: uuid::Uuid::new_v4().to_string(),
        parent_uuid: None,
        logical_parent_uuid: None,
        session_id: Some(session_id.clone()),
        cwd: cwd.display().to_string(),
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        version: None,
        git_branch: None,
        is_sidechain: false,
        agent_id: None,
        message: Some(serde_json::json!({
            "role": "user",
            "content": [{"type": "text", "text": "seed"}],
        })),
        usage: None,
        model: Some("seed-model".to_string()),
        request_id: None,
        cost_usd: None,
        extra: serde_json::Map::new(),
    };
    store
        .append_message(session_id.as_str(), &entry)
        .expect("seed transcript");
}

#[tokio::test]
async fn prompt_mode_bash_responds_by_default() {
    let home = TempDir::new().unwrap();
    let registry = coco_commands::CommandRegistry::new();
    let runtime = build_runtime_with_registry(&home, registry).await;

    assert!(coco_agent_host::session_controls::should_respond_to_bash_commands(&runtime));
}

#[tokio::test]
async fn prompt_mode_bash_returns_response_turn_to_driver_channel() {
    let home = TempDir::new().unwrap();
    let registry = coco_commands::CommandRegistry::new();
    let runtime = build_runtime_with_registry(&home, registry).await;
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(4);
    let (response_tx, mut response_rx) = tokio::sync::mpsc::channel(1);

    run_prompt_mode_bash(
        home.path(),
        "bash-message-id".to_string(),
        "printf prompt-bash".to_string(),
        runtime,
        event_tx,
        response_tx,
    )
    .await;

    let mut saw_completion = false;
    for _ in 0..4 {
        let event = tokio::time::timeout(Duration::from_secs(1), event_rx.recv())
            .await
            .expect("bash completion event should be sent")
            .expect("event channel should stay open");
        if let coco_types::CoreEvent::Tui(coco_types::TuiOnlyEvent::BashCommandCompleted {
            user_message_id,
            output,
            exit_code,
        }) = event
        {
            assert_eq!(user_message_id, "bash-message-id");
            assert_eq!(exit_code, 0);
            assert!(output.contains("prompt-bash"));
            saw_completion = true;
            break;
        }
    }
    assert!(saw_completion, "expected BashCommandCompleted event");

    let response_messages = tokio::time::timeout(Duration::from_secs(1), response_rx.recv())
        .await
        .expect("bash response turn should be returned")
        .expect("response channel should stay open");
    assert!(
        !response_messages.is_empty(),
        "bash response turn should include history override messages"
    );
}

#[tokio::test]
async fn prompt_mode_bash_respects_respond_setting_false() {
    let home = TempDir::new().unwrap();
    let registry = coco_commands::CommandRegistry::new();
    let runtime = build_runtime_with_registry_and_settings(
        &home,
        registry,
        Settings {
            respond_to_bash_commands: Some(false),
            ..Default::default()
        },
    )
    .await;

    assert!(!coco_agent_host::session_controls::should_respond_to_bash_commands(&runtime));
}

#[tokio::test]
async fn idle_queue_processor_starts_pending_prompt_turn() {
    let home = TempDir::new().unwrap();
    let registry = coco_commands::CommandRegistry::new();
    let runtime = build_runtime_with_registry(&home, registry).await;
    let queued = coco_query::QueuedCommand::new(
        "queued follow-up after text response".into(),
        coco_query::QueuePriority::Next,
    )
    .with_origin(coco_system_reminder::QueueOrigin::Human);
    let queued_id = queued.id.to_string();
    runtime.command_queue().enqueue(queued).await;

    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(64);
    let active_turn = Arc::new(Mutex::new(None));
    let (turn_done_tx, mut turn_done_rx) = tokio::sync::mpsc::channel(4);
    let mut pending_editor_requests = std::collections::HashMap::new();
    let title_gen_attempted = Arc::new(tokio::sync::RwLock::new(std::collections::HashSet::new()));
    let mut local_app_server_bridge = coco_agent_host::app_server_host::AppServerLocalBridge::new(
        Arc::new(coco_agent_host::app_server_host::AppServerHostState::default()),
    );
    install_test_session_runtime(&mut local_app_server_bridge, &runtime).await;
    let (current_session, _runtime_factory, _process_runtime, _cwd) =
        test_resume_context(&runtime).await;
    let reload_subscriptions = test_runtime_reload_subscriptions(&current_session, &event_tx);

    process_idle_command_queue(
        &runtime,
        &current_session,
        &event_tx,
        &mut local_app_server_bridge,
        &active_turn,
        &mut pending_editor_requests,
        &title_gen_attempted,
        &turn_done_tx,
        &reload_subscriptions,
    )
    .await;

    assert!(
        runtime.command_queue().is_empty().await,
        "queued prompt should be consumed into a follow-up turn"
    );
    assert!(
        active_turn.lock().await.is_some(),
        "queued prompt should start a follow-up turn"
    );

    let completed_turn = tokio::time::timeout(Duration::from_secs(1), turn_done_rx.recv())
        .await
        .expect("queued follow-up turn should finish")
        .expect("turn_done channel should stay open");
    assert!(drain_completed_turn(&active_turn, completed_turn).await);
    assert_eq!(
        runtime.last_assistant_text().await.as_deref(),
        Some("queued turn complete"),
        "queued prompt should complete through the local AppServer turn path"
    );

    let mut saw_dequeued = false;
    let mut saw_queue_empty = false;
    while let Ok(event) = event_rx.try_recv() {
        match event {
            coco_types::CoreEvent::Protocol(coco_types::ServerNotification::CommandDequeued {
                id,
            }) if id == queued_id => {
                saw_dequeued = true;
            }
            coco_types::CoreEvent::Protocol(
                coco_types::ServerNotification::QueueStateChanged { queued: 0 },
            ) => {
                saw_queue_empty = true;
            }
            _ => {}
        }
    }
    assert!(saw_dequeued, "queued prompt should emit CommandDequeued");
    assert!(
        saw_queue_empty,
        "queued prompt should emit QueueStateChanged queued=0"
    );
}

#[tokio::test]
async fn local_app_server_turn_writes_back_runtime_history() {
    let home = TempDir::new().unwrap();
    let registry = coco_commands::CommandRegistry::new();
    let runtime = build_runtime_with_registry(&home, registry).await;
    let session_id = runtime.current_typed_session_id().await;
    let mut local_app_server_bridge = coco_agent_host::app_server_host::AppServerLocalBridge::new(
        Arc::new(coco_agent_host::app_server_host::AppServerHostState::default()),
    );
    install_test_session_runtime(&mut local_app_server_bridge, &runtime).await;

    let completion = local_app_server_bridge
        .start_turn_and_wait_for_end(
            session_id.clone(),
            coco_types::TurnStartParams {
                target: coco_types::InteractiveTarget {
                    session_id: session_id.clone(),
                    surface_id: coco_types::SurfaceId::generate(),
                },
                prompt: "write back runtime history".into(),
                history_override: Vec::new(),
                images: Vec::new(),
                composer: Default::default(),
                slash_metadata: None,
                model_selection: None,
                permission_mode: None,
                thinking_level: None,
                goal_continuation: false,
            },
        )
        .await
        .expect("local AppServer turn completes");

    assert_eq!(completion.ended.turn_id, completion.started.turn_id);
    assert!(
        completion.ended.session_result.is_some(),
        "TurnEnded should carry the per-turn result on AppServer local completions"
    );
    assert_eq!(completion.session_result.total_turns, 1);
    assert_eq!(
        completion.session_result.result.as_deref(),
        Some("queued turn complete")
    );
    assert_eq!(
        runtime.last_assistant_text().await.as_deref(),
        Some("queued turn complete")
    );
}

#[tokio::test]
async fn manual_compact_uses_local_app_server_turn_shortcut() {
    let home = TempDir::new().unwrap();
    let registry = coco_commands::CommandRegistry::new();
    let runtime = build_runtime_with_registry(&home, registry).await;
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(32);
    let active_turn = Arc::new(Mutex::new(None));
    let (turn_done_tx, mut turn_done_rx) = tokio::sync::mpsc::channel(4);
    let mut local_app_server_bridge = coco_agent_host::app_server_host::AppServerLocalBridge::new(
        Arc::new(coco_agent_host::app_server_host::AppServerHostState::default()),
    );
    install_test_session_runtime(&mut local_app_server_bridge, &runtime).await;

    run_manual_compact(
        &runtime,
        &event_tx,
        &mut local_app_server_bridge,
        Some("focus auth".to_string()),
        &active_turn,
        &turn_done_tx,
    )
    .await;

    assert!(
        active_turn.lock().await.is_some(),
        "manual compact should start an AppServer-owned active turn"
    );
    let completed_turn = tokio::time::timeout(Duration::from_secs(3), turn_done_rx.recv())
        .await
        .expect("manual compact turn should finish")
        .expect("turn_done channel should stay open");
    assert!(drain_completed_turn(&active_turn, completed_turn).await);
    let messages = runtime.history_messages().await;
    assert_eq!(messages.len(), 2);
    let echo = coco_messages::wrapping::extract_text_from_message(&messages[0]);
    let result = coco_messages::wrapping::extract_text_from_message(&messages[1]);
    assert!(echo.contains("/compact"));
    assert!(echo.contains("focus auth"));
    assert!(result.contains("No messages to compact."));
    assert!(!echo.contains(coco_commands::handlers::compact::COMPACT_SENTINEL));
    assert!(!result.contains(coco_commands::handlers::compact::COMPACT_SENTINEL));
}

#[tokio::test]
async fn btw_dispatch_supports_open_only_and_preserves_images() {
    let home = TempDir::new().unwrap();
    let mut registry = coco_commands::CommandRegistry::new();
    coco_commands::register_extended_builtins(&mut registry);
    let runtime = build_runtime_with_registry(&home, registry).await;
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
    let (current_session, _factory, _process_runtime, _cwd) = test_resume_context(&runtime).await;
    let reload_subscriptions = test_runtime_reload_subscriptions(&current_session, &event_tx);
    let mut bridge = coco_agent_host::app_server_host::AppServerLocalBridge::new(Arc::new(
        coco_agent_host::app_server_host::AppServerHostState::default(),
    ));
    install_test_session_runtime(&mut bridge, &runtime).await;

    let open = super::dispatch_slash_command(
        "btw",
        "",
        &runtime,
        &current_session,
        &event_tx,
        &mut bridge,
        &reload_subscriptions,
        &[],
    )
    .await;
    assert!(matches!(
        open,
        super::SlashOutcome::TriggerBtw {
            request: coco_commands::handlers::btw::BtwRequest::Open,
            images
        } if images.is_empty()
    ));

    let input_images = vec![coco_types::QueuedCommandEditImage {
        media_type: "image/png".to_string(),
        data_base64: "aW1hZ2U=".to_string(),
        insertion_offset: 0,
    }];
    let ask = super::dispatch_slash_command(
        "btw",
        "inspect this",
        &runtime,
        &current_session,
        &event_tx,
        &mut bridge,
        &reload_subscriptions,
        &input_images,
    )
    .await;
    assert!(matches!(
        ask,
        super::SlashOutcome::TriggerBtw {
            request: coco_commands::handlers::btw::BtwRequest::OpenAndAsk { question },
            images,
        } if question == "inspect this" && images == input_images
    ));
}

#[tokio::test]
async fn sidechat_dispatch_rejects_nested_and_primary_only_slashes() {
    let home = TempDir::new().unwrap();
    let mut registry = coco_commands::CommandRegistry::new();
    coco_commands::register_extended_builtins(&mut registry);
    let parent = build_runtime_with_registry(&home, registry).await;
    let parent_id = parent.current_typed_session_id().await;
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(32);
    let (current_session, factory, process_runtime, _cwd) = test_resume_context(&parent).await;
    let reload_subscriptions = test_runtime_reload_subscriptions(&current_session, &event_tx);
    let mut bridge =
        local_bridge_for_resume_test(&parent, factory, process_runtime, Some(event_tx.clone()));
    install_test_session_runtime(&mut bridge, &parent).await;
    let child = bridge
        .open_side_chat(parent_id, Some(event_tx.clone()))
        .await
        .expect("open sidechat");
    let child_id = child.session.current_typed_session_id().await;

    for name in ["btw", "help"] {
        let outcome = super::dispatch_slash_command(
            name,
            "",
            &child.session,
            &current_session,
            &event_tx,
            &mut bridge,
            &reload_subscriptions,
            &[],
        )
        .await;
        assert!(matches!(outcome, super::SlashOutcome::Handled));

        let result = loop {
            let event = tokio::time::timeout(Duration::from_secs(3), event_rx.recv())
                .await
                .expect("sidechat rejection should be emitted")
                .expect("event channel should remain open");
            if let coco_types::CoreEvent::Tui(
                result @ coco_types::TuiOnlyEvent::SlashCommandResult { .. },
            ) = event
            {
                break result;
            }
        };
        let coco_types::TuiOnlyEvent::SlashCommandResult {
            session_id,
            name: emitted_name,
            text,
            ..
        } = result
        else {
            unreachable!();
        };
        assert_eq!(session_id, child_id);
        assert_eq!(emitted_name, name);
        assert!(text.contains("only /compact and /context"));
    }
}

#[tokio::test]
async fn btw_does_not_disturb_active_parent_turn() {
    // `/btw` now creates an independent read-only sidechat child with its own
    // turn coordinator, so it never touches the parent's active turn. With no
    // runtime factory configured in this test bridge, it surfaces a graceful
    // error instead of disturbing the parent.
    struct DropFlag(Arc<AtomicBool>);
    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    let home = TempDir::new().unwrap();
    let registry = coco_commands::CommandRegistry::new();
    let runtime = build_runtime_with_registry(&home, registry).await;
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(32);
    let (turn_done_tx, _turn_done_rx) = tokio::sync::mpsc::channel(4);
    let state = Arc::new(coco_agent_host::app_server_host::AppServerHostState::default());
    let mut local_app_server_bridge =
        coco_agent_host::app_server_host::AppServerLocalBridge::new(Arc::clone(&state));
    install_test_session_runtime(&mut local_app_server_bridge, &runtime).await;

    // A live parent turn: a pending task parked in the active-turn slot.
    let dropped = Arc::new(AtomicBool::new(false));
    let dropped_for_task = dropped.clone();
    let parent_task = tokio::spawn(async move {
        let _guard = DropFlag(dropped_for_task);
        std::future::pending::<()>().await;
    });
    let parent_turn_id = uuid::Uuid::new_v4();
    let active_turn = Arc::new(Mutex::new(Some(ActiveTurn {
        id: parent_turn_id,
        task: parent_task,
        cancel: ActiveTurnCancel {
            client: local_app_server_bridge.connect_local_client(),
            handler: local_app_server_bridge.handler().clone(),
            target: coco_types::InteractiveTarget {
                session_id: coco_types::SessionId::generate(),
                surface_id: coco_types::SurfaceId::generate(),
            },
        },
    })));

    run_side_chat(
        &runtime,
        &event_tx,
        &mut local_app_server_bridge,
        &active_turn,
        &turn_done_tx,
        coco_commands::handlers::btw::BtwRequest::OpenAndAsk {
            question: "how does caching work?".to_string(),
        },
        &[],
    )
    .await;

    // The parent turn is untouched: same slot, same id, task never aborted.
    {
        let guard = active_turn.lock().await;
        let parent = guard.as_ref().expect("parent turn slot must survive /btw");
        assert_eq!(parent.id, parent_turn_id);
        assert!(
            !parent.task.is_finished(),
            "parent turn task must not be touched by /btw"
        );
    }
    assert!(
        !dropped.load(Ordering::SeqCst),
        "parent turn task must not be dropped/aborted by /btw"
    );

    // Without a runtime factory the sidechat can't be built; the failure is
    // surfaced gracefully (after the view enter/exit events) rather than
    // silently swallowed.
    let mut surfaced = false;
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(1), event_rx.recv()).await
    {
        if let coco_types::CoreEvent::Tui(coco_types::TuiOnlyEvent::SlashCommandResult {
            name,
            text,
            ..
        }) = event
        {
            assert_eq!(name, "btw");
            assert!(text.contains("Couldn't start the sidechat"));
            surfaced = true;
            break;
        }
    }
    assert!(
        surfaced,
        "the sidechat failure should be surfaced as a slash-command result"
    );
}

#[tokio::test]
async fn side_chat_runtime_installs_read_only_tool_boundary() {
    // A runtime built with the SideChatReadOnly profile installs the structural
    // read-only tool gate on every turn and sets require_can_use_tool so a
    // PreToolUse hook auto-approve cannot bypass it. A Primary runtime does not.
    let home = TempDir::new().unwrap();
    let side_chat = build_runtime_with_profile(
        &home,
        coco_commands::CommandRegistry::new(),
        Settings::default(),
        crate::session_runtime::SessionExecutionProfile::SideChatReadOnly,
    )
    .await;
    let cfg = side_chat.current_engine_config().await;
    assert!(
        cfg.can_use_tool.is_some(),
        "sidechat runtime must install the read-only tool gate"
    );
    assert!(
        cfg.require_can_use_tool,
        "sidechat must set require_can_use_tool so hook auto-approve cannot bypass the boundary"
    );

    let home2 = TempDir::new().unwrap();
    let primary = build_runtime_with_registry(&home2, coco_commands::CommandRegistry::new()).await;
    let primary_cfg = primary.current_engine_config().await;
    assert!(
        primary_cfg.can_use_tool.is_none(),
        "a primary runtime installs no can_use_tool gate"
    );
    assert!(!primary_cfg.require_can_use_tool);
}

#[tokio::test]
async fn side_chat_runtime_accounts_usage_without_persisting_transcript() {
    let home = TempDir::new().unwrap();
    let runtime = build_runtime_with_profile(
        &home,
        coco_commands::CommandRegistry::new(),
        Settings::default(),
        crate::session_runtime::SessionExecutionProfile::SideChatReadOnly,
    )
    .await;
    let session_id = runtime.current_typed_session_id().await;
    let mut bridge = coco_agent_host::app_server_host::AppServerLocalBridge::new(Arc::new(
        coco_agent_host::app_server_host::AppServerHostState::default(),
    ));
    install_test_session_runtime(&mut bridge, &runtime).await;

    bridge
        .start_turn_and_wait_for_end(
            session_id.clone(),
            coco_types::TurnStartParams {
                target: coco_types::InteractiveTarget {
                    session_id,
                    surface_id: coco_types::SurfaceId::generate(),
                },
                prompt: "account this ephemeral sidechat turn".into(),
                history_override: Vec::new(),
                images: Vec::new(),
                composer: Default::default(),
                slash_metadata: None,
                model_selection: None,
                permission_mode: None,
                thinking_level: None,
                goal_continuation: false,
            },
        )
        .await
        .expect("sidechat turn completes");

    let usage = runtime.session_usage_snapshot().await;
    assert_eq!(usage.totals.input_tokens, 1);
    assert_eq!(usage.totals.output_tokens, 1);
}

#[tokio::test]
async fn side_chat_factory_routes_rolls_up_persists_and_restores_usage() {
    let home = TempDir::new().unwrap();
    let mut settings = Settings::default();
    settings.session.backend = coco_config::SessionBackend::Memory;
    let mut registry = coco_commands::CommandRegistry::new();
    coco_commands::register_extended_builtins(&mut registry);
    let parent = build_runtime_with_registry_and_settings(&home, registry, settings).await;
    let parent_id = parent.current_typed_session_id().await;
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(128);
    parent.install_side_query_event_tx(event_tx.clone()).await;
    let (_current_session, factory, process_runtime, _cwd) = test_resume_context(&parent).await;
    let mut bridge =
        local_bridge_for_resume_test(&parent, factory, process_runtime, Some(event_tx.clone()));
    install_test_session_runtime(&mut bridge, &parent).await;

    // Establish durable main-session usage before the child exists.
    bridge
        .start_turn_and_wait_for_end(
            parent_id.clone(),
            coco_types::TurnStartParams {
                target: coco_types::InteractiveTarget {
                    session_id: parent_id.clone(),
                    surface_id: coco_types::SurfaceId::generate(),
                },
                prompt: "main turn".into(),
                history_override: Vec::new(),
                images: Vec::new(),
                composer: Default::default(),
                slash_metadata: None,
                model_selection: None,
                permission_mode: None,
                thinking_level: None,
                goal_continuation: false,
            },
        )
        .await
        .expect("parent turn completes");
    let parent_history_len = parent.history_messages().await.len();
    let parent_usage_before = parent.session_usage_snapshot().await;
    assert_eq!(parent_usage_before.totals.input_tokens, 1);
    assert_eq!(parent_usage_before.totals.output_tokens, 1);

    // This is the production factory/registry/child-pump path, not a
    // separately constructed SideChatReadOnly runtime.
    let child = bridge
        .open_side_chat(parent_id.clone(), Some(event_tx.clone()))
        .await
        .expect("sidechat factory should build and attach a child");
    let child_id = child.session.current_typed_session_id().await;
    let child_surface = bridge
        .interactive_session_by_id(&child_id)
        .expect("child surface must resolve by its exact session id");
    assert_eq!(child_surface.session_id(), &child_id);
    assert_ne!(
        bridge
            .interactive_session()
            .expect("primary surface remains attached")
            .session_id(),
        &child_id,
        "the generic interactive surface is primary and must not drive child follow-ups"
    );
    let child_registry = child.session.current_command_registry().await;
    let mut child_commands: Vec<&str> = child_registry
        .all()
        .map(|command| command.base.name.as_str())
        .collect();
    child_commands.sort_unstable();
    assert_eq!(child_commands, vec!["compact", "context"]);
    let parent_registry = parent.current_command_registry().await;
    assert!(
        !Arc::ptr_eq(&parent_registry, &child_registry),
        "the sidechat must own an isolated command registry projection"
    );
    let parent_config = parent.current_engine_config().await;
    let child_config = child.session.current_engine_config().await;
    assert_eq!(child_config.model_id, parent_config.model_id);
    assert_eq!(child_config.thinking_level, parent_config.thinking_level);
    assert_eq!(child_config.permission_mode, parent_config.permission_mode);
    assert_eq!(child_config.fast_mode, parent_config.fast_mode);
    assert!(child_config.can_use_tool.is_some());
    assert!(child_config.require_can_use_tool);

    let mut tui_state = coco_tui::AppState::new();
    tui_state.session.session_id = Some(parent_id.to_string());
    tui_state.session.session_usage = Some(parent_usage_before);
    assert!(tui_state.enter_side_chat(parent_id.clone(), child_id.clone()));
    let (command_tx, _command_rx) = tokio::sync::mpsc::channel::<coco_tui::UserCommand>(64);

    bridge
        .start_child_turn(coco_types::TurnStartParams {
            target: coco_types::InteractiveTarget {
                session_id: child_id.clone(),
                surface_id: coco_types::SurfaceId::generate(),
            },
            prompt: "sidechat turn".into(),
            history_override: Vec::new(),
            images: Vec::new(),
            composer: Default::default(),
            slash_metadata: None,
            model_selection: None,
            permission_mode: None,
            thinking_level: None,
            goal_continuation: false,
        })
        .await
        .expect("child turn should start");

    let mut saw_parent_rollup_event = false;
    let mut saw_child_usage_event = false;
    let mut saw_child_turn_end = false;
    for _ in 0..128 {
        let event = tokio::time::timeout(Duration::from_secs(3), event_rx.recv())
            .await
            .expect("child event stream should remain live")
            .expect("child event channel should remain open");
        if let coco_types::CoreEvent::Tui(coco_types::TuiOnlyEvent::SessionScoped {
            session_id,
            event: scoped,
        }) = &event
            && let coco_types::SessionScopedEvent::Protocol(notification) = scoped.as_ref()
        {
            match notification.as_ref() {
                coco_types::ServerNotification::SessionUsageUpdated(snapshot)
                    if session_id == &parent_id =>
                {
                    saw_parent_rollup_event = true;
                    assert_eq!(snapshot.session_id, parent_id);
                }
                coco_types::ServerNotification::SessionUsageUpdated(snapshot)
                    if session_id == &child_id =>
                {
                    saw_child_usage_event = true;
                    assert_eq!(snapshot.session_id, child_id);
                }
                coco_types::ServerNotification::TurnEnded(_) if session_id == &child_id => {
                    saw_child_turn_end = true;
                }
                _ => {}
            }
        }
        coco_tui::handle_core_event(&mut tui_state, event, &command_tx);
        assert!(
            tui_state
                .session
                .session_usage
                .as_ref()
                .is_none_or(|usage| usage.session_id == child_id),
            "a parent usage event must never overwrite the active child projection"
        );
        if saw_child_turn_end {
            break;
        }
    }
    assert!(saw_parent_rollup_event);
    assert!(saw_child_usage_event);
    assert!(saw_child_turn_end);

    let child_usage = child.session.session_usage_snapshot().await;
    assert_eq!(child_usage.totals.input_tokens, 1);
    assert_eq!(child_usage.totals.output_tokens, 1);
    let parent_usage = parent.session_usage_snapshot().await;
    assert_eq!(parent_usage.totals.input_tokens, 2);
    assert_eq!(parent_usage.totals.output_tokens, 2);
    assert!(parent_usage.source_records.iter().any(|entry| {
        entry.source == coco_types::UsageSource::Main
            && entry.input_tokens == 1
            && entry.output_tokens == 1
    }));
    assert!(parent_usage.source_records.iter().any(|entry| {
        entry.source == coco_types::UsageSource::SideQuery
            && entry.input_tokens == 1
            && entry.output_tokens == 1
    }));
    assert_eq!(
        parent.history_messages().await.len(),
        parent_history_len,
        "sidechat must not mutate the parent transcript"
    );

    let store = parent
        .session_manager_handle()
        .store_for(parent.original_cwd());
    let persisted_parent = store
        .load_usage_snapshot(parent_id.as_str())
        .expect("load parent usage")
        .expect("parent usage should be persisted");
    assert_eq!(persisted_parent.totals, parent_usage.totals);
    assert_eq!(persisted_parent.source_records, parent_usage.source_records);
    assert!(
        store
            .load_usage_snapshot(child_id.as_str())
            .expect("load child usage")
            .is_none(),
        "ephemeral child usage must not create its own persisted ledger"
    );
    assert!(!store.exists(child_id.as_str()));

    bridge
        .close_child()
        .await
        .expect("sidechat close should complete");
    let mut saw_final_parent_snapshot = false;
    let mut saw_exit = false;
    for _ in 0..32 {
        let event = tokio::time::timeout(Duration::from_secs(3), event_rx.recv())
            .await
            .expect("close lifecycle should publish its terminal events")
            .expect("event channel should remain open");
        if let coco_types::CoreEvent::Tui(coco_types::TuiOnlyEvent::SessionScoped {
            session_id,
            event: scoped,
        }) = &event
            && session_id == &parent_id
            && matches!(
                scoped.as_ref(),
                coco_types::SessionScopedEvent::Protocol(notification)
                    if matches!(notification.as_ref(), coco_types::ServerNotification::SessionUsageUpdated(_))
            )
        {
            saw_final_parent_snapshot = true;
        }
        if matches!(
            &event,
            coco_types::CoreEvent::Tui(coco_types::TuiOnlyEvent::SideChatExited {
                parent_id: event_parent,
                child_id: event_child,
            }) if event_parent == &parent_id && event_child == &child_id
        ) {
            assert!(
                saw_final_parent_snapshot,
                "the authoritative parent snapshot must precede view restoration"
            );
            saw_exit = true;
        }
        coco_tui::handle_core_event(&mut tui_state, event, &command_tx);
        if saw_exit {
            break;
        }
    }
    assert!(saw_exit);
    assert!(!tui_state.is_viewing_side_chat());
    assert_eq!(
        tui_state
            .session
            .session_usage
            .as_ref()
            .expect("restored parent usage")
            .totals,
        parent_usage.totals
    );
}

#[tokio::test]
async fn autonomous_side_chat_close_refreshes_parent_and_releases_bridge_authority() {
    let home = TempDir::new().unwrap();
    let parent = build_runtime_with_registry(&home, coco_commands::CommandRegistry::new()).await;
    let parent_id = parent.current_typed_session_id().await;
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(32);
    let (_current_session, factory, process_runtime, _cwd) = test_resume_context(&parent).await;
    let mut bridge =
        local_bridge_for_resume_test(&parent, factory, process_runtime, Some(event_tx.clone()));
    install_test_session_runtime(&mut bridge, &parent).await;
    let child = bridge
        .open_side_chat(parent_id.clone(), Some(event_tx.clone()))
        .await
        .expect("open sidechat");
    let child_id = child.session.current_typed_session_id().await;

    // Close through a separate client, bypassing bridge.close_child. This is
    // the autonomous/error lifecycle path that still must restore the TUI and
    // stop advertising the terminal child as live.
    bridge
        .client()
        .session_close(
            bridge.handler(),
            coco_types::SessionCloseParams {
                target: coco_types::SessionCloseTarget::Interactive {
                    target: child.surface.interactive_target(),
                },
            },
        )
        .await
        .expect("autonomous child close");

    let mut saw_parent_snapshot = false;
    let mut saw_exit = false;
    for _ in 0..16 {
        let event = tokio::time::timeout(Duration::from_secs(3), event_rx.recv())
            .await
            .expect("lifecycle event should arrive")
            .expect("event channel should remain open");
        match event {
            coco_types::CoreEvent::Tui(coco_types::TuiOnlyEvent::SessionScoped {
                session_id,
                event,
            }) if session_id == parent_id
                && matches!(
                    event.as_ref(),
                    coco_types::SessionScopedEvent::Protocol(notification)
                        if matches!(notification.as_ref(), coco_types::ServerNotification::SessionUsageUpdated(_))
                ) =>
            {
                saw_parent_snapshot = true;
            }
            coco_types::CoreEvent::Tui(coco_types::TuiOnlyEvent::SideChatExited {
                parent_id: event_parent,
                child_id: event_child,
            }) if event_parent == parent_id && event_child == child_id => {
                assert!(saw_parent_snapshot);
                saw_exit = true;
                break;
            }
            _ => {}
        }
    }
    assert!(saw_exit);
    assert!(
        bridge.child_interactive_session().is_none(),
        "a terminal registry slot must not retain bridge authority"
    );

    let replacement = bridge
        .open_side_chat(parent_id, Some(event_tx))
        .await
        .expect("a new sidechat should open after autonomous termination");
    assert_ne!(replacement.session.session_id(), &child_id);
    bridge.close_child().await.expect("close replacement child");
}

#[tokio::test]
async fn capture_side_chat_seed_bounds_committed_parent_history() {
    // The parent captures its committed history as bounded inherited context
    // for a sidechat child. Under a large budget this is the full prefix.
    let home = TempDir::new().unwrap();
    let parent = build_runtime_with_registry(&home, coco_commands::CommandRegistry::new()).await;
    parent
        .append_messages_to_history(vec![
            coco_messages::create_user_message("what does the cache layer do?"),
            coco_messages::create_user_message("and how is it invalidated?"),
        ])
        .await;

    let seed = parent
        .capture_side_chat_seed()
        .await
        .expect("capture parent seed");
    assert!(seed.context().fidelity().is_full_prefix());
    assert!(
        seed.context().messages().len() >= 2,
        "seed should carry the committed parent turns"
    );
    assert!(seed.context().estimated_tokens() > 0);
}

#[tokio::test]
async fn memory_shortcuts_use_local_app_server_turn_shortcuts() {
    let home = TempDir::new().unwrap();
    let registry = coco_commands::CommandRegistry::new();
    let runtime = build_runtime_with_registry(&home, registry).await;
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(32);
    let active_turn = Arc::new(Mutex::new(None));
    let (turn_done_tx, mut turn_done_rx) = tokio::sync::mpsc::channel(4);
    let mut local_app_server_bridge = coco_agent_host::app_server_host::AppServerLocalBridge::new(
        Arc::new(coco_agent_host::app_server_host::AppServerHostState::default()),
    );
    install_test_session_runtime(&mut local_app_server_bridge, &runtime).await;

    run_dream_consolidation(
        &runtime,
        &event_tx,
        &mut local_app_server_bridge,
        &active_turn,
        &turn_done_tx,
    )
    .await;
    assert!(
        active_turn.lock().await.is_some(),
        "/dream should start an AppServer-owned active turn"
    );
    let completed_turn = tokio::time::timeout(Duration::from_secs(3), turn_done_rx.recv())
        .await
        .expect("/dream turn should finish")
        .expect("turn_done channel should stay open");
    assert!(drain_completed_turn(&active_turn, completed_turn).await);

    run_session_memory_force(
        &runtime,
        &event_tx,
        &mut local_app_server_bridge,
        &active_turn,
        &turn_done_tx,
    )
    .await;
    assert!(
        active_turn.lock().await.is_some(),
        "/summary should start an AppServer-owned active turn"
    );
    let completed_turn = tokio::time::timeout(Duration::from_secs(3), turn_done_rx.recv())
        .await
        .expect("/summary turn should finish")
        .expect("turn_done channel should stay open");
    assert!(drain_completed_turn(&active_turn, completed_turn).await);

    assert!(
        runtime.history_messages().await.is_empty(),
        "memory shortcut no-op path should not append sentinel text"
    );
}

#[tokio::test]
async fn local_app_server_bridge_uses_runtime_session_manager_for_session_list() {
    let home = TempDir::new().unwrap();
    let registry = coco_commands::CommandRegistry::new();
    let runtime = build_runtime_with_registry(&home, registry).await;
    let session_id = runtime.current_typed_session_id().await;
    seed_runtime_session_transcript(&runtime).await;
    let mut local_app_server_bridge = coco_agent_host::app_server_host::AppServerLocalBridge::new(
        Arc::new(coco_agent_host::app_server_host::AppServerHostState::default()),
    );
    install_test_session_runtime(&mut local_app_server_bridge, &runtime).await;

    let listed = local_app_server_bridge
        .client()
        .session_list(local_app_server_bridge.handler())
        .await
        .expect("local session/list succeeds");

    assert!(
        listed
            .sessions
            .iter()
            .any(|session| session.session_id == session_id),
        "local AppServer session/list should read persisted runtime sessions"
    );
}

#[tokio::test]
async fn local_app_server_bridge_uses_runtime_session_manager_for_session_read() {
    let home = TempDir::new().unwrap();
    let registry = coco_commands::CommandRegistry::new();
    let runtime = build_runtime_with_registry(&home, registry).await;
    let session_id = runtime.current_typed_session_id().await;
    seed_runtime_session_transcript(&runtime).await;
    let mut local_app_server_bridge = coco_agent_host::app_server_host::AppServerLocalBridge::new(
        Arc::new(coco_agent_host::app_server_host::AppServerHostState::default()),
    );
    install_test_session_runtime(&mut local_app_server_bridge, &runtime).await;

    let read = local_app_server_bridge
        .client()
        .session_read(
            local_app_server_bridge.handler(),
            coco_types::SessionReadParams {
                target: coco_types::SessionTarget {
                    session_id: session_id.clone(),
                },
                cursor: None,
                limit: None,
            },
        )
        .await
        .expect("local session/read succeeds");

    assert_eq!(read.session.session_id, session_id);
    assert_eq!(read.messages.len(), 1);
    assert_eq!(read.messages[0]["message"]["content"][0]["text"], "seed");
    assert!(!read.has_more);
}

#[tokio::test]
async fn local_app_server_bridge_reads_live_runtime_handle_history() {
    let home = TempDir::new().unwrap();
    let registry = coco_commands::CommandRegistry::new();
    let runtime = build_runtime_with_registry(&home, registry).await;
    let session_id = runtime.current_typed_session_id().await;
    let mut local_app_server_bridge = coco_agent_host::app_server_host::AppServerLocalBridge::new(
        Arc::new(coco_agent_host::app_server_host::AppServerHostState::default()),
    );
    install_test_session_runtime(&mut local_app_server_bridge, &runtime).await;

    runtime
        .append_messages_to_history(vec![coco_messages::create_user_message(
            "live runtime only",
        )])
        .await;

    let read = local_app_server_bridge
        .client()
        .session_read(
            local_app_server_bridge.handler(),
            coco_types::SessionReadParams {
                target: coco_types::SessionTarget {
                    session_id: session_id.clone(),
                },
                cursor: None,
                limit: None,
            },
        )
        .await
        .expect("local live session/read succeeds");

    assert_eq!(read.session.session_id, session_id);
    assert_eq!(read.messages.len(), 1);
    assert_eq!(
        read.messages[0]["message"]["content"][0]["text"],
        "live runtime only"
    );
    assert!(!read.has_more);
}

#[tokio::test]
async fn startup_resume_plan_uses_local_app_server_session_resume() {
    let home = TempDir::new().unwrap();
    let registry = coco_commands::CommandRegistry::new();
    let runtime = build_runtime_with_registry(&home, registry).await;
    let old_session_id = runtime.current_typed_session_id().await;
    let target_session_id =
        coco_types::SessionId::try_new("sess-tui-startup-resume-target").expect("valid session id");
    let rt = &runtime;
    seed_session_transcript_for_cwd(
        rt.session_manager_handle().memory_base(),
        rt.original_cwd(),
        &target_session_id,
    );
    let project_store = coco_session::TranscriptStore::new(coco_agent_host::paths::project_paths(
        rt.original_cwd(),
    ));
    append_seed_transcript(&project_store, rt.original_cwd(), &target_session_id);
    let plan = coco_agent_host::runtime_resume::load_resume_plan_for_runtime_target(
        &runtime,
        target_session_id.as_str(),
    )
    .await
    .expect("load resume plan");

    let (current_session, runtime_factory, process_runtime, _cwd) =
        test_resume_context(&runtime).await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let mut local_app_server_bridge = local_bridge_for_resume_test(
        &runtime,
        runtime_factory.clone(),
        Arc::clone(&process_runtime),
        Some(tx.clone()),
    );
    install_test_session_runtime(&mut local_app_server_bridge, &runtime).await;
    local_app_server_bridge
        .ensure_interactive_surface(old_session_id)
        .expect("attach old surface");
    let reload_subscriptions = test_runtime_reload_subscriptions(&current_session, &tx);

    apply_resume_plan_through_app_server(
        &plan,
        &current_session,
        &tx,
        &mut local_app_server_bridge,
        &reload_subscriptions,
    )
    .await
    .expect("startup resume through AppServer");

    assert_eq!(
        current_session
            .read()
            .await
            .current_typed_session_id()
            .await,
        target_session_id
    );
    let live = local_app_server_bridge.app_server().list_live_sessions();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].session_id, target_session_id);

    let mut saw_reset = false;
    let mut saw_history = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            coco_types::CoreEvent::Protocol(
                coco_types::ServerNotification::SessionResetForResume { identity },
            ) if identity.session_id.as_ref() == Some(&target_session_id) => {
                saw_reset = true;
            }
            coco_types::CoreEvent::Protocol(coco_types::ServerNotification::HistoryReplaced {
                identity,
                ..
            }) if identity.session_id.as_ref() == Some(&target_session_id) => {
                saw_history = true;
            }
            _ => {}
        }
    }
    assert!(saw_reset, "startup resume should emit TUI reset event");
    assert!(
        saw_history,
        "startup resume should emit TUI history replacement"
    );
}

#[tokio::test]
async fn resume_slash_uses_local_app_server_session_resume() {
    let home = TempDir::new().unwrap();
    let registry = coco_commands::CommandRegistry::new();
    let runtime = build_runtime_with_registry(&home, registry).await;
    let old_session_id = runtime.current_typed_session_id().await;
    let target_session_id =
        coco_types::SessionId::try_new("sess-tui-resume-target").expect("valid session id");
    let rt = &runtime;
    seed_session_transcript_for_cwd(
        rt.session_manager_handle().memory_base(),
        rt.original_cwd(),
        &target_session_id,
    );
    let project_store = coco_session::TranscriptStore::new(coco_agent_host::paths::project_paths(
        rt.original_cwd(),
    ));
    append_seed_transcript(&project_store, rt.original_cwd(), &target_session_id);

    let (current_session, runtime_factory, process_runtime, _cwd) =
        test_resume_context(&runtime).await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let mut local_app_server_bridge = local_bridge_for_resume_test(
        &runtime,
        runtime_factory.clone(),
        Arc::clone(&process_runtime),
        Some(tx.clone()),
    );
    install_test_session_runtime(&mut local_app_server_bridge, &runtime).await;
    local_app_server_bridge
        .ensure_interactive_surface(old_session_id)
        .expect("attach old surface");
    let reload_subscriptions = test_runtime_reload_subscriptions(&current_session, &tx);

    let outcome = dispatch_slash_command(
        "resume",
        target_session_id.as_str(),
        &runtime,
        &current_session,
        &tx,
        &mut local_app_server_bridge,
        &reload_subscriptions,
        &[],
    )
    .await;

    assert!(matches!(outcome, super::SlashOutcome::Handled));
    let events = {
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        events
    };
    assert_eq!(
        current_session
            .read()
            .await
            .current_typed_session_id()
            .await,
        target_session_id,
        "resume events: {events:?}"
    );
    let live = local_app_server_bridge.app_server().list_live_sessions();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].session_id, target_session_id);

    let mut saw_reset = false;
    let mut saw_history = false;
    for event in events {
        match event {
            coco_types::CoreEvent::Protocol(
                coco_types::ServerNotification::SessionResetForResume { identity },
            ) if identity.session_id.as_ref() == Some(&target_session_id) => {
                saw_reset = true;
            }
            coco_types::CoreEvent::Protocol(coco_types::ServerNotification::HistoryReplaced {
                identity,
                ..
            }) if identity.session_id.as_ref() == Some(&target_session_id) => {
                saw_history = true;
            }
            _ => {}
        }
    }
    assert!(saw_reset, "resume should emit TUI reset event");
    assert!(saw_history, "resume should emit TUI history replacement");
}

#[tokio::test]
async fn branch_slash_switches_to_fork_through_local_app_server() {
    let home = TempDir::new().unwrap();
    let registry = coco_commands::CommandRegistry::new();
    let runtime = build_runtime_with_registry(&home, registry).await;
    let old_session_id = runtime.current_typed_session_id().await;
    seed_runtime_session_transcript(&runtime).await;

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let (current_session, runtime_factory, process_runtime, _cwd) =
        test_resume_context(&runtime).await;
    let mut local_app_server_bridge = local_bridge_for_resume_test(
        &runtime,
        runtime_factory.clone(),
        Arc::clone(&process_runtime),
        Some(tx.clone()),
    );
    install_test_session_runtime(&mut local_app_server_bridge, &runtime).await;
    local_app_server_bridge
        .ensure_interactive_surface(old_session_id.clone())
        .expect("attach old surface");
    let reload_subscriptions = test_runtime_reload_subscriptions(&current_session, &tx);

    let outcome = dispatch_slash_command(
        "branch",
        "test fork",
        &runtime,
        &current_session,
        &tx,
        &mut local_app_server_bridge,
        &reload_subscriptions,
        &[],
    )
    .await;

    assert!(matches!(outcome, super::SlashOutcome::Handled));
    let new_session = current_session.read().await.clone();
    let new_session_id = new_session.current_typed_session_id().await;
    assert_ne!(new_session_id, old_session_id);
    let live = local_app_server_bridge.app_server().list_live_sessions();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].session_id, new_session_id);
    let forked_session = new_session
        .session_manager_handle()
        .load(new_session_id.as_str())
        .expect("branch title should persist through local AppServer session/rename");
    assert_eq!(forked_session.title.as_deref(), Some("test fork (Branch)"));

    let mut saw_reset = false;
    let mut saw_history = false;
    let mut saw_branch_result = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            coco_types::CoreEvent::Protocol(
                coco_types::ServerNotification::SessionResetForResume { identity },
            ) if identity.session_id.as_ref() == Some(&new_session_id) => {
                saw_reset = true;
            }
            coco_types::CoreEvent::Protocol(coco_types::ServerNotification::HistoryReplaced {
                identity,
                ..
            }) if identity.session_id.as_ref() == Some(&new_session_id) => {
                saw_history = true;
            }
            coco_types::CoreEvent::Tui(coco_types::TuiOnlyEvent::SlashCommandResult {
                name,
                text,
                ..
            }) if name == "branch" && text.contains("Branched into a new session") => {
                saw_branch_result = true;
            }
            _ => {}
        }
    }
    assert!(saw_reset, "branch should emit TUI reset event");
    assert!(saw_history, "branch should emit TUI history replacement");
    assert!(saw_branch_result, "branch should report the forked session");
}

#[tokio::test]
async fn clear_slash_refreshes_local_app_server_session() {
    let home = TempDir::new().unwrap();
    let registry = coco_commands::CommandRegistry::new();
    let runtime = build_runtime_with_registry(&home, registry).await;
    let old_session_id = runtime.current_typed_session_id().await;
    let (current_session, runtime_factory, process_runtime, _cwd) =
        test_resume_context(&runtime).await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let reload_subscriptions = test_runtime_reload_subscriptions(&current_session, &tx);
    let active_turn = Arc::new(Mutex::new(None));
    let mut local_app_server_bridge = local_bridge_for_resume_test(
        &runtime,
        runtime_factory.clone(),
        Arc::clone(&process_runtime),
        Some(tx.clone()),
    );
    install_test_session_runtime(&mut local_app_server_bridge, &runtime).await;
    local_app_server_bridge
        .ensure_interactive_surface(old_session_id.clone())
        .expect("attach old surface");

    let (turn_done_tx, _turn_done_rx) = tokio::sync::mpsc::channel(1);
    let clear_context = LocalRuntimeControlContext {
        current_session: &current_session,
        runtime_reload_subscriptions: &reload_subscriptions,
        turn_done_tx: &turn_done_tx,
    };
    run_clear_conversation(
        &runtime,
        &clear_context,
        &active_turn,
        &tx,
        &mut local_app_server_bridge,
    )
    .await;

    let current = current_session.read().await.clone();
    let new_session_id = current.current_typed_session_id().await;
    assert_ne!(new_session_id, old_session_id);
    assert_eq!(runtime.current_typed_session_id().await, old_session_id);
    let live = local_app_server_bridge.app_server().list_live_sessions();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].session_id, new_session_id);

    let mut saw_reset = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            coco_types::CoreEvent::Protocol(
                coco_types::ServerNotification::SessionResetForResume { identity },
            ) if identity.session_id.as_ref() == Some(&new_session_id) => {
                saw_reset = true;
            }
            _ => {}
        }
    }
    assert!(saw_reset, "/clear should emit TUI reset event");
}

#[tokio::test]
async fn rename_and_tag_slashes_use_local_app_server_session_metadata_controls() {
    let home = TempDir::new().unwrap();
    let registry = coco_commands::CommandRegistry::new();
    let runtime = build_runtime_with_registry(&home, registry).await;
    let session_id = runtime.current_typed_session_id().await;
    seed_runtime_session_transcript(&runtime).await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let mut local_app_server_bridge = coco_agent_host::app_server_host::AppServerLocalBridge::new(
        Arc::new(coco_agent_host::app_server_host::AppServerHostState::default()),
    );
    install_test_session_runtime(&mut local_app_server_bridge, &runtime).await;

    run_session_rename(
        &runtime,
        &tx,
        &local_app_server_bridge,
        coco_commands::ParsedRename::Explicit("phase-b-cleanup".to_string()),
    )
    .await;
    run_session_tag(&runtime, &tx, &local_app_server_bridge, "phase-b").await;

    let session = runtime
        .session_manager_handle()
        .load(session_id.as_str())
        .expect("metadata controls should persist session updates");
    assert_eq!(session.title.as_deref(), Some("phase-b-cleanup"));
    assert_eq!(session.tags, vec!["phase-b".to_string()]);

    let mut saw_rename = false;
    let mut saw_tag = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            coco_types::CoreEvent::Tui(coco_types::TuiOnlyEvent::SlashCommandResult {
                name,
                text,
                ..
            }) if name == "rename" && text == "Session renamed to: phase-b-cleanup" => {
                saw_rename = true;
            }
            coco_types::CoreEvent::Tui(coco_types::TuiOnlyEvent::SlashCommandResult {
                name,
                text,
                ..
            }) if name == "tag" && text == "Tag added: phase-b" => {
                saw_tag = true;
            }
            _ => {}
        }
    }
    assert!(saw_rename, "rename should report success");
    assert!(saw_tag, "tag should report success");
}

#[tokio::test]
async fn cost_and_status_slashes_use_local_app_server_observability() {
    let home = TempDir::new().unwrap();
    let registry = coco_commands::CommandRegistry::new();
    let runtime = build_runtime_with_registry(&home, registry).await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let mut local_app_server_bridge = coco_agent_host::app_server_host::AppServerLocalBridge::new(
        Arc::new(coco_agent_host::app_server_host::AppServerHostState::default()),
    );
    install_test_session_runtime(&mut local_app_server_bridge, &runtime).await;

    run_show_cost(&runtime, &tx, &local_app_server_bridge).await;
    run_show_status(&runtime, &tx, &local_app_server_bridge).await;

    let mut saw_cost = false;
    let mut saw_status = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            coco_types::CoreEvent::Tui(coco_types::TuiOnlyEvent::SlashCommandResult {
                name,
                text,
                ..
            }) if name == "cost" && text.contains("No API usage recorded yet") => {
                saw_cost = true;
            }
            coco_types::CoreEvent::Tui(coco_types::TuiOnlyEvent::OpenGoalStatus {
                title,
                body,
                ..
            }) if title == "Status" && body.contains("Session status:") => {
                saw_status = true;
            }
            _ => {}
        }
    }
    assert!(
        saw_cost,
        "cost should render from local AppServer session/cost"
    );
    assert!(
        saw_status,
        "status should render from local AppServer session/status"
    );
}

#[tokio::test]
async fn tasks_list_and_detail_slashes_use_local_app_server_task_observability() {
    let home = TempDir::new().unwrap();
    let registry = coco_commands::CommandRegistry::new();
    let runtime = build_runtime_with_registry(&home, registry).await;
    let task_runtime = Arc::new(coco_agent_host::task_runtime::TaskRuntime::new(Arc::new(
        coco_tasks::TaskManager::new(),
    )));
    let task_id = task_runtime
        .register_agent_task(
            "background work",
            None,
            None,
            tokio_util::sync::CancellationToken::new(),
            coco_tool_runtime::AgentRegistration::Background,
        )
        .await;
    runtime.attach_task_runtime(Arc::clone(&task_runtime)).await;

    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let mut local_app_server_bridge = coco_agent_host::app_server_host::AppServerLocalBridge::new(
        Arc::new(coco_agent_host::app_server_host::AppServerHostState::default()),
    );
    install_test_session_runtime(&mut local_app_server_bridge, &runtime).await;

    let list_outcome = dispatch_slash_command_for_test(
        "tasks",
        "list",
        &runtime,
        &tx,
        &mut local_app_server_bridge,
    )
    .await;
    let detail_outcome = dispatch_slash_command_for_test(
        "tasks",
        &format!("detail {task_id}"),
        &runtime,
        &tx,
        &mut local_app_server_bridge,
    )
    .await;

    assert!(matches!(list_outcome, super::SlashOutcome::Handled));
    assert!(matches!(detail_outcome, super::SlashOutcome::Handled));

    let mut saw_list = false;
    let mut saw_detail = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            coco_types::CoreEvent::Tui(coco_types::TuiOnlyEvent::SlashCommandResult {
                name,
                args,
                text,
                ..
            }) if name == "tasks"
                && args == "list"
                && text.contains(&task_id)
                && text.contains("background work") =>
            {
                saw_list = true;
            }
            coco_types::CoreEvent::Tui(coco_types::TuiOnlyEvent::SlashCommandResult {
                name,
                args,
                text,
                ..
            }) if name == "tasks"
                && args == format!("detail {task_id}")
                && text.contains(&format!("Task {task_id}"))
                && text.contains("Interrupted: false") =>
            {
                saw_detail = true;
            }
            _ => {}
        }
    }

    assert!(
        saw_list,
        "tasks list should render from local AppServer task/list"
    );
    assert!(
        saw_detail,
        "tasks detail should render from local AppServer task/detail"
    );
}

#[tokio::test]
async fn background_all_tasks_uses_local_app_server_control() {
    let home = TempDir::new().unwrap();
    let registry = coco_commands::CommandRegistry::new();
    let runtime = build_runtime_with_registry(&home, registry).await;
    let task_runtime = Arc::new(coco_agent_host::task_runtime::TaskRuntime::new(Arc::new(
        coco_tasks::TaskManager::new(),
    )));
    let task_id = task_runtime
        .register_agent_task(
            "foreground work",
            None,
            None,
            tokio_util::sync::CancellationToken::new(),
            coco_tool_runtime::AgentRegistration::Foreground,
        )
        .await;
    runtime.attach_task_runtime(Arc::clone(&task_runtime)).await;

    let mut local_app_server_bridge = coco_agent_host::app_server_host::AppServerLocalBridge::new(
        Arc::new(coco_agent_host::app_server_host::AppServerHostState::default()),
    );
    install_test_session_runtime(&mut local_app_server_bridge, &runtime).await;

    let task_ids = background_all_tasks_through_app_server(&runtime, &mut local_app_server_bridge)
        .await
        .expect("background-all should dispatch through local AppServer");

    assert_eq!(task_ids, vec![task_id.clone()]);
    let state = task_runtime
        .manager()
        .get(&task_id)
        .await
        .expect("task should remain registered");
    assert!(state.is_backgrounded());
}

#[tokio::test]
async fn tasks_cancel_slash_uses_local_app_server_stop_task() {
    let home = TempDir::new().unwrap();
    let registry = coco_commands::CommandRegistry::new();
    let runtime = build_runtime_with_registry(&home, registry).await;
    let task_runtime = Arc::new(coco_agent_host::task_runtime::TaskRuntime::new(Arc::new(
        coco_tasks::TaskManager::new(),
    )));
    let cancel = tokio_util::sync::CancellationToken::new();
    let task_id = task_runtime
        .register_agent_task(
            "background work",
            None,
            None,
            cancel.clone(),
            coco_tool_runtime::AgentRegistration::Background,
        )
        .await;
    runtime.attach_task_runtime(Arc::clone(&task_runtime)).await;

    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    let mut local_app_server_bridge = coco_agent_host::app_server_host::AppServerLocalBridge::new(
        Arc::new(coco_agent_host::app_server_host::AppServerHostState::default()),
    );
    install_test_session_runtime(&mut local_app_server_bridge, &runtime).await;

    let outcome = dispatch_slash_command_for_test(
        "tasks",
        &format!("cancel {task_id}"),
        &runtime,
        &tx,
        &mut local_app_server_bridge,
    )
    .await;

    assert!(matches!(outcome, super::SlashOutcome::Handled));
    assert!(cancel.is_cancelled(), "task cancel token should fire");
    let event = rx.recv().await.expect("slash result event");
    match event {
        coco_types::CoreEvent::Tui(coco_types::TuiOnlyEvent::SlashCommandResult {
            name,
            args,
            text,
            ..
        }) => {
            assert_eq!(name, "tasks");
            assert_eq!(args, format!("cancel {task_id}"));
            assert_eq!(text, format!("Cancelled task {task_id}."));
        }
        other => panic!("expected slash result, got {other:?}"),
    }
}

#[tokio::test]
async fn toggle_fast_mode_uses_local_app_server_apply_flags() {
    let home = TempDir::new().unwrap();
    let registry = coco_commands::CommandRegistry::new();
    let runtime = build_runtime_with_registry(&home, registry).await;
    assert!(!runtime.current_engine_config().await.fast_mode);

    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    let mut local_app_server_bridge = coco_agent_host::app_server_host::AppServerLocalBridge::new(
        Arc::new(coco_agent_host::app_server_host::AppServerHostState::default()),
    );
    install_test_session_runtime(&mut local_app_server_bridge, &runtime).await;

    toggle_fast_mode_through_app_server(&runtime, &tx, &mut local_app_server_bridge).await;

    assert!(runtime.current_engine_config().await.fast_mode);
    let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("fast-mode event should be forwarded")
        .expect("event channel should stay open");
    let (session_id, notification) = expect_session_scoped_protocol(event);
    assert_eq!(session_id, *runtime.session_id());
    match notification {
        coco_types::ServerNotification::FastModeChanged { active } => {
            assert!(active);
        }
        other => panic!("expected FastModeChanged, got {other:?}"),
    }
}

#[tokio::test]
async fn set_thinking_level_uses_local_app_server_set_thinking() {
    let home = TempDir::new().unwrap();
    let registry = coco_commands::CommandRegistry::new();
    let runtime = build_runtime_with_registry(&home, registry).await;
    assert!(
        runtime
            .current_engine_config()
            .await
            .thinking_level
            .is_none()
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    let mut local_app_server_bridge = coco_agent_host::app_server_host::AppServerLocalBridge::new(
        Arc::new(coco_agent_host::app_server_host::AppServerHostState::default()),
    );
    install_test_session_runtime(&mut local_app_server_bridge, &runtime).await;

    set_thinking_level_through_app_server(
        &runtime,
        &tx,
        &mut local_app_server_bridge,
        "high".to_string(),
    )
    .await;

    let cfg = runtime.current_engine_config().await;
    let thinking = cfg.thinking_level.expect("runtime thinking level");
    assert_eq!(thinking.effort, coco_types::ReasoningEffort::High);
    let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("model-role event should be forwarded")
        .expect("event channel should stay open");
    let (session_id, notification) = expect_session_scoped_protocol(event);
    assert_eq!(session_id, *runtime.session_id());
    match notification {
        coco_types::ServerNotification::ModelRoleChanged(params) => {
            assert_eq!(params.role, coco_types::ModelRole::Main);
            assert_eq!(params.effort, Some(coco_types::ReasoningEffort::High));
        }
        other => panic!("expected ModelRoleChanged, got {other:?}"),
    }
}

#[tokio::test]
async fn explicit_file_rewind_restores_files_through_local_app_server() {
    let home = TempDir::new().unwrap();
    let registry = coco_commands::CommandRegistry::new();
    let runtime = build_runtime_with_registry_and_settings(
        &home,
        registry,
        Settings {
            file_checkpointing_enabled: true,
            ..Default::default()
        },
    )
    .await;
    let rt = &runtime;
    let file = home.path().join("rewind.txt");
    tokio::fs::write(&file, "original\n").await.unwrap();
    rt.record_file_edit_for_rewind(&file, "msg-1")
        .await
        .expect("track file edit");
    tokio::fs::write(&file, "modified\n").await.unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    let mut local_app_server_bridge = coco_agent_host::app_server_host::AppServerLocalBridge::new(
        Arc::new(coco_agent_host::app_server_host::AppServerHostState::default()),
    );
    install_test_session_runtime(&mut local_app_server_bridge, &runtime).await;

    handle_rewind(
        &coco_tui::state::RestoreType::CodeOnly,
        "msg-1",
        /*rewound_turn*/ 1,
        &tx,
        &runtime,
        &mut local_app_server_bridge,
    )
    .await;

    assert_eq!(
        tokio::fs::read_to_string(&file).await.unwrap(),
        "original\n"
    );
    let event = rx.recv().await.expect("rewind completed event");
    match event {
        coco_types::CoreEvent::Tui(coco_types::TuiOnlyEvent::RewindCompleted {
            target_message_id,
            files_changed,
        }) => {
            assert_eq!(target_message_id, "");
            assert_eq!(files_changed, 1);
        }
        other => panic!("expected RewindCompleted, got {other:?}"),
    }
    while let Ok(event) = rx.try_recv() {
        if matches!(
            event,
            coco_types::CoreEvent::Protocol(coco_types::ServerNotification::Error(_))
        ) {
            panic!("successful file rewind should not emit an error: {event:?}");
        }
    }
}

#[tokio::test]
async fn model_slash_arg_rejects_unavailable_model() {
    let mut registry = coco_commands::CommandRegistry::new();
    coco_commands::register_extended_builtins(&mut registry);
    let home = TempDir::new().expect("tempdir");
    let runtime = build_runtime_with_registry_and_settings(
        &home,
        registry,
        Settings {
            available_models: Some(vec!["anthropic/claude-opus-4-7".to_string()]),
            ..Default::default()
        },
    )
    .await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    let mut local_app_server_bridge = coco_agent_host::app_server_host::AppServerLocalBridge::new(
        Arc::new(coco_agent_host::app_server_host::AppServerHostState::default()),
    );
    install_test_session_runtime(&mut local_app_server_bridge, &runtime).await;

    let outcome = dispatch_slash_command_for_test(
        "model",
        "gpt5",
        &runtime,
        &tx,
        &mut local_app_server_bridge,
    )
    .await;

    assert!(matches!(outcome, super::SlashOutcome::Handled));
    let event = rx.recv().await.expect("slash result event");
    match event {
        coco_types::CoreEvent::Tui(coco_types::TuiOnlyEvent::SlashCommandResult {
            name,
            args,
            text,
            ..
        }) => {
            assert_eq!(name, "model");
            assert_eq!(args, "gpt5");
            assert!(text.contains("restricted by your organization's settings"));
            assert!(text.contains("Run /model to choose a different model"));
            assert!(text.contains("gpt-5-4"));
        }
        other => panic!("expected slash result, got {other:?}"),
    }
}

#[tokio::test]
async fn inactive_slash_command_emits_session_hint_without_running_handler() {
    let mut registry = coco_commands::CommandRegistry::new();
    registry.register(coco_commands::RegisteredCommand {
        base: CommandBase {
            name: "blocked".to_string(),
            description: "inactive command".to_string(),
            aliases: Vec::new(),
            availability: Vec::new(),
            is_hidden: false,
            argument_hint: None,
            argument_kind: CommandArgumentKind::None,
            when_to_use: None,
            user_invocable: true,
            is_sensitive: false,
            loaded_from: Some(CommandSource::Builtin),
            skill_badge: None,
            safety: CommandSafety::AlwaysSafe,
            supports_non_interactive: false,
            session_scope: coco_types::SlashCommandSessionScope::PrimaryOnly,
        },
        command_type: CommandType::Local(LocalCommandData {
            handler: "blocked".to_string(),
        }),
        handler: Some(Arc::new(coco_commands::BuiltinCommand::new(
            "blocked",
            inactive_test_command_handler,
        ))),
        is_enabled: Some(inactive_test_command_enabled),
    });
    let home = TempDir::new().expect("tempdir");
    let runtime = build_runtime_with_registry(&home, registry).await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    let mut local_app_server_bridge = coco_agent_host::app_server_host::AppServerLocalBridge::new(
        Arc::new(coco_agent_host::app_server_host::AppServerHostState::default()),
    );
    install_test_session_runtime(&mut local_app_server_bridge, &runtime).await;

    let outcome = dispatch_slash_command_for_test(
        "blocked",
        "arg",
        &runtime,
        &tx,
        &mut local_app_server_bridge,
    )
    .await;

    assert!(matches!(outcome, super::SlashOutcome::Handled));
    let event = rx.recv().await.expect("slash result event");
    match event {
        coco_types::CoreEvent::Tui(coco_types::TuiOnlyEvent::SlashCommandResult {
            name,
            args,
            text,
            ..
        }) => {
            assert_eq!(name, "blocked");
            assert_eq!(args, "arg");
            assert_eq!(text, slash_unavailable_in_session_message("blocked"));
            assert_ne!(text, "handler should not run");
        }
        other => panic!("expected slash result, got {other:?}"),
    }
}

#[test]
fn slash_prompt_metadata_matches_ts_shape() {
    assert_eq!(
        coco_agent_host::session_messages::slash_command_metadata("simplify", "focus on tests"),
        "<command-message>simplify</command-message>\n\
         <command-name>/simplify</command-name>\n\
         <command-args>focus on tests</command-args>"
    );
}

#[test]
fn slash_prompt_metadata_omits_empty_args() {
    assert_eq!(
        coco_agent_host::session_messages::slash_command_metadata("simplify", ""),
        "<command-message>simplify</command-message>\n\
         <command-name>/simplify</command-name>"
    );
}

#[test]
fn slash_prompt_metadata_message_has_distinct_identity_and_kind() {
    let metadata = coco_agent_host::session_messages::slash_command_metadata("simplify", "focus");
    let message = coco_agent_host::session_messages::slash_metadata_message(&metadata);
    let coco_messages::Message::Attachment(attachment) = message else {
        panic!("slash metadata should be an attachment");
    };
    assert_eq!(
        attachment.kind,
        coco_types::AttachmentKind::SlashCommandMetadata
    );
    assert_ne!(attachment.uuid, uuid::Uuid::nil());
}

#[test]
fn session_plan_file_path_uses_runtime_plan_directory_setting() {
    let config_home = tempfile::tempdir().expect("config home");
    let project = tempfile::tempdir().expect("project");
    let path = session_plan_file_path(
        config_home.path(),
        Some(project.path()),
        Some("plans"),
        &match coco_types::SessionId::try_new("session-1") {
            Ok(id) => id,
            Err(_) => unreachable!("test session id must be valid"),
        },
    );

    assert!(path.starts_with(project.path().canonicalize().unwrap().join("plans")));
    assert_eq!(path.extension().and_then(|e| e.to_str()), Some("md"));
}

#[test]
fn parse_editor_command_splits_quoted_args() {
    let (program, args) =
        parse_editor_command("code --wait --reuse-window 'memory file.md'").expect("parsed");
    assert_eq!(program, "code");
    assert_eq!(args, vec!["--wait", "--reuse-window", "memory file.md"]);
}

#[test]
fn parse_editor_command_rejects_unbalanced_quotes() {
    let err = parse_editor_command("code 'unterminated").expect_err("should reject");
    assert!(err.contains("failed to parse editor command"));
}

// `classify_sentinel_trigger` — decides whether a registry handler's
// Text output is actually a request to fire a real feature (compact /
// dream / summary). Wrong classification means the user's `/compact`
// would silently print sentinel garbage instead of triggering compaction.

#[test]
fn classify_sentinel_compact_no_args() {
    use coco_commands::handlers::compact::COMPACT_SENTINEL;
    let text = format!("{COMPACT_SENTINEL} \nCompacting conversation…\n");
    assert_eq!(
        classify_sentinel_trigger(&text),
        Some(SentinelTrigger::Compact {
            custom_instructions: None
        })
    );
}

#[test]
fn classify_sentinel_compact_with_instructions() {
    use coco_commands::handlers::compact::COMPACT_SENTINEL;
    let text = format!("{COMPACT_SENTINEL} focus on auth\nCompacting…\n");
    assert_eq!(
        classify_sentinel_trigger(&text),
        Some(SentinelTrigger::Compact {
            custom_instructions: Some("focus on auth".to_string()),
        })
    );
}

#[test]
fn classify_sentinel_compact_whitespace_only_args_treated_as_none() {
    use coco_commands::handlers::compact::COMPACT_SENTINEL;
    // The handler emits "{SENTINEL}  \n" when args is whitespace; trim
    // should fold that back to None so the engine doesn't see an empty
    // custom_instructions string.
    let text = format!("{COMPACT_SENTINEL}    \nCompacting…\n");
    assert_eq!(
        classify_sentinel_trigger(&text),
        Some(SentinelTrigger::Compact {
            custom_instructions: None
        })
    );
}

#[test]
fn classify_sentinel_dream() {
    use coco_commands::handlers::dream::DREAM_SENTINEL;
    let text = format!("{DREAM_SENTINEL} \nKAIROS dream consolidation…\n");
    assert_eq!(
        classify_sentinel_trigger(&text),
        Some(SentinelTrigger::Dream)
    );
}

#[test]
fn first_user_prompt_title_uses_first_line_truncated() {
    use coco_messages::create_user_message;
    let msgs = vec![create_user_message("Build the auth flow\nmore detail")];
    assert_eq!(
        super::first_user_prompt_title(&msgs).as_deref(),
        Some("Build the auth flow")
    );
}

#[test]
fn classify_sentinel_summary() {
    use coco_commands::handlers::summary::SUMMARY_SENTINEL;
    let text = format!("{SUMMARY_SENTINEL} \nWriting session memory…\n");
    assert_eq!(
        classify_sentinel_trigger(&text),
        Some(SentinelTrigger::Summary)
    );
}

#[test]
fn classify_sentinel_plain_text_returns_none() {
    // The vast majority of handler outputs — anything not starting with
    // a sentinel — must classify as None so the dispatcher renders them
    // verbatim in the transcript.
    assert_eq!(classify_sentinel_trigger(""), None);
    assert_eq!(classify_sentinel_trigger("Hello, world"), None);
    assert_eq!(
        classify_sentinel_trigger("## Permission Rules\n\nNo rules"),
        None
    );
}

#[test]
fn classify_sentinel_does_not_match_substring() {
    // Sentinels must be at the *start*; a sentinel embedded in body text
    // (e.g. echoed inside an explanation) must not trigger.
    use coco_commands::handlers::compact::COMPACT_SENTINEL;
    let text = format!("Here is the sentinel: {COMPACT_SENTINEL}");
    assert_eq!(classify_sentinel_trigger(&text), None);
}

// `parse_permissions_mutation` — distinguishes the read-only / list
// path (None, falls through to registry) from the three mutating
// subcommands the TUI dispatcher actually applies to engine_config.

#[test]
fn parse_permissions_reset() {
    assert_eq!(
        parse_permissions_mutation("reset"),
        Some(PermissionsMutation::Reset)
    );
    assert_eq!(
        parse_permissions_mutation("  reset  "),
        Some(PermissionsMutation::Reset)
    );
}

#[test]
fn parse_permissions_allow() {
    assert_eq!(
        parse_permissions_mutation("allow Bash"),
        Some(PermissionsMutation::Allow("Bash".to_string()))
    );
    assert_eq!(
        parse_permissions_mutation("allow mcp__server__tool"),
        Some(PermissionsMutation::Allow("mcp__server__tool".to_string()))
    );
}

#[test]
fn parse_permissions_deny() {
    assert_eq!(
        parse_permissions_mutation("deny Write"),
        Some(PermissionsMutation::Deny("Write".to_string()))
    );
}

#[test]
fn parse_permissions_list_falls_through_to_registry() {
    // The read-only paths return None so the dispatcher hands off to
    // the registry handler (which reads settings.json and renders).
    assert_eq!(parse_permissions_mutation(""), None);
    assert_eq!(parse_permissions_mutation("list"), None);
    assert_eq!(parse_permissions_mutation("  "), None);
}

#[test]
fn parse_permissions_allow_without_tool_is_none() {
    // `allow ` with no tool name must fall through (the dispatcher then
    // emits a usage hint) — never let an empty-string tool reach
    // engine_config.allow_rules.
    assert_eq!(parse_permissions_mutation("allow"), None);
    assert_eq!(parse_permissions_mutation("allow "), None);
    assert_eq!(parse_permissions_mutation("allow   "), None);
}

#[test]
fn parse_permissions_deny_without_tool_is_none() {
    assert_eq!(parse_permissions_mutation("deny"), None);
    assert_eq!(parse_permissions_mutation("deny "), None);
}

#[test]
fn parse_permissions_unknown_subcommand_is_none() {
    // Unknown words pass through to the registry handler, which renders
    // its own "Unknown permissions subcommand" error.
    assert_eq!(parse_permissions_mutation("foobar"), None);
    assert_eq!(parse_permissions_mutation("revoke Bash"), None);
}

#[test]
fn title_gen_exhaustive_truth_table() {
    // Exhaustive check: the gate returns true for EXACTLY the all-true
    // combination and false for every other combination. Catches any
    // future refactor that accidentally makes one condition optional.
    for auto in [false, true] {
        for already in [false, true] {
            for spec in [false, true] {
                for exited in [false, true] {
                    for plan in [false, true] {
                        let result = should_trigger_title_gen(auto, already, spec, exited, plan);
                        let expected = auto && !already && spec && exited && plan;
                        assert_eq!(
                            result, expected,
                            "auto={auto} already={already} spec={spec} exited={exited} plan={plan}"
                        );
                    }
                }
            }
        }
    }
}

// ── /plan dispatch ──
//
// `dispatch_plan` itself talks to a `SessionRuntime` and is exercised
// by integration tests. The rule "fire a query if and only if `args`
// is non-empty AND not 'open'" is encoded in
// `plan_command_query_after_flip` as a pure helper so we cover that
// regression-prone branch without spinning up the runtime.

use super::plan_command_query_after_flip;

#[test]
fn plan_query_after_flip_fires_for_real_description() {
    assert_eq!(
        plan_command_query_after_flip("refactor the auth flow"),
        Some("refactor the auth flow")
    );
}

#[test]
fn plan_query_after_flip_trims_whitespace() {
    assert_eq!(
        plan_command_query_after_flip("   refactor   "),
        Some("refactor")
    );
}

#[test]
fn plan_query_after_flip_skips_bare_plan() {
    // Bare `/plan` (empty args) calls `onDone ('Enabled plan mode')`
    // WITHOUT `shouldQuery`.
    assert_eq!(plan_command_query_after_flip(""), None);
    assert_eq!(plan_command_query_after_flip("   "), None);
}

#[test]
fn plan_query_after_flip_skips_open_subcommand() {
    // `description !== 'open'` filter — `/plan open` opens an editor,
    // never fires a query.
    assert_eq!(plan_command_query_after_flip("open"), None);
    assert_eq!(plan_command_query_after_flip("  open  "), None);
}

#[test]
fn plan_query_after_flip_open_substring_still_queries() {
    // Only the bare token "open" suppresses the query — descriptions
    // that happen to contain it must still query.
    assert_eq!(
        plan_command_query_after_flip("open the door"),
        Some("open the door")
    );
}

mod truncate_output_tests {
    use super::super::truncate_output;
    use pretty_assertions::assert_eq;

    #[test]
    fn short_text_passes_through() {
        assert_eq!(truncate_output("hello".into(), 100, 10), "hello");
    }

    #[test]
    fn caps_at_line_count_with_marker() {
        let text = (0..15)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = truncate_output(text, 10_000, 5);
        let lines = out.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 6);
        assert_eq!(lines[0], "line 0");
        assert_eq!(lines[4], "line 4");
        assert_eq!(lines[5], "… (truncated)");
    }

    #[test]
    fn caps_at_byte_budget() {
        let long = "x".repeat(500);
        let out = truncate_output(long, 50, 1000);
        assert!(out.starts_with(&"x".repeat(50)));
        assert!(out.ends_with("(truncated)"));
    }

    #[test]
    fn preserves_utf8_boundaries_when_cut() {
        // Each 🚀 is 4 bytes; 60 chars × 4 = 240 bytes. The byte cut
        // must land on a 4-byte boundary so the string stays valid
        // UTF-8 (`.chars().count()` panics on a malformed slice).
        let rocket_run: String = "🚀".repeat(60);
        let out = truncate_output(rocket_run, 100, 1000);
        let _ = out.chars().count();
        assert!(out.ends_with("(truncated)"));
    }
}

mod turn_done_guard_tests {
    use super::super::TurnDoneGuard;
    use pretty_assertions::assert_eq;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn fires_on_normal_scope_exit() {
        let (tx, mut rx) = mpsc::channel::<uuid::Uuid>(4);
        let id = uuid::Uuid::new_v4();
        {
            let _guard = TurnDoneGuard {
                turn_id: id,
                tx: tx.clone(),
            };
        }
        assert_eq!(rx.recv().await, Some(id));
    }

    #[tokio::test]
    async fn fires_on_panic_unwind_inside_spawn() {
        // The bug we're guarding against: a spawned turn task panics
        // before the original tail `turn_done_tx.send(...)` runs, so
        // the completion signal never fires and `active_turn` stays
        // locked. Drop runs during unwind, so the guard must signal
        // even on panic.
        let (tx, mut rx) = mpsc::channel::<uuid::Uuid>(4);
        let id = uuid::Uuid::new_v4();
        let tx_t = tx.clone();
        let prior_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let handle = tokio::spawn(async move {
            let _guard = TurnDoneGuard {
                turn_id: id,
                tx: tx_t,
            };
            panic!("intentional turn-task panic for test");
        });
        let res = handle.await;
        std::panic::set_hook(prior_hook);
        assert!(res.is_err(), "spawned task should have surfaced JoinError");
        assert_eq!(rx.recv().await, Some(id));
    }
}

mod turn_already_running_detection_tests {
    use super::super::is_turn_already_running;
    use coco_app_server_client::ClientError;

    #[test]
    fn detects_turn_already_running_server_error() {
        let error = ClientError::Server {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "a turn is already running; call turn/interrupt first".to_string(),
            data: None,
        };
        assert!(is_turn_already_running(&error));
    }

    #[test]
    fn ignores_no_active_session_sharing_the_same_code() {
        // NoActiveSession also maps to INVALID_REQUEST; it must not be
        // treated as retryable.
        let error = ClientError::Server {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: "no active session; call session/start first".to_string(),
            data: None,
        };
        assert!(!is_turn_already_running(&error));
    }

    #[test]
    fn ignores_matching_message_under_wrong_code() {
        let error = ClientError::Server {
            code: coco_types::error_codes::INTERNAL_ERROR,
            message: "a turn is already running".to_string(),
            data: None,
        };
        assert!(!is_turn_already_running(&error));
    }

    #[test]
    fn ignores_non_server_variants() {
        assert!(!is_turn_already_running(&ClientError::SnapshotRequired));
    }
}
