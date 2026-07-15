use std::{future::Future, pin::Pin, sync::Arc};

use coco_app_server::{
    JsonRpcConnectionHandlerFactory, JsonRpcRequestContext, JsonRpcRequestHandler,
};
use coco_types::{ClientRequest, CoreEvent, InitializeParams, RequestScope, ServerNotification};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::app_server_host::TurnRunner;

use super::*;

struct NoResultTurnRunner;

impl TurnRunner for NoResultTurnRunner {
    fn run_turn<'a>(
        &'a self,
        _session: crate::session_runtime::SessionHandle,
        _app_server: Arc<coco_app_server::AppServer<crate::app_session::AppSessionHandle>>,
        _params: coco_types::TurnStartParams,
        turn_id: coco_types::TurnId,
        event_tx: mpsc::Sender<CoreEvent>,
        _cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async move {
            event_tx
                .send(CoreEvent::Protocol(ServerNotification::TurnStarted(
                    coco_types::TurnStartedParams {
                        turn_id: turn_id.clone(),
                    },
                )))
                .await?;
            event_tx
                .send(CoreEvent::Protocol(ServerNotification::TurnEnded(
                    coco_types::TurnEndedParams::completed(
                        turn_id,
                        Some(coco_types::TokenUsage::default()),
                        None,
                    ),
                )))
                .await?;
            Ok(())
        })
    }
}

#[tokio::test]
async fn connection_factory_owns_independent_initialize_state() {
    let state = Arc::new(AppServerHostState::default());
    let (notif_tx, _notif_rx) = mpsc::channel(8);
    let factory = AppServerHostHandler::new(state, notif_tx);
    let connection_a = coco_app_server::ConnectionKey::generate();
    let connection_b = coco_app_server::ConnectionKey::generate();
    let handler_a = factory.open(connection_a);
    let handler_b = factory.open(connection_b);

    for (handler, connection) in [(&handler_a, connection_a), (&handler_b, connection_b)] {
        handler
            .handle_json_rpc_request(
                JsonRpcRequestContext {
                    connection,
                    scope: RequestScope::Connection,
                },
                ClientRequest::Initialize(InitializeParams::default()),
            )
            .await
            .expect("first initialize succeeds");
    }

    let duplicate = handler_a
        .handle_json_rpc_request(
            JsonRpcRequestContext {
                connection: connection_a,
                scope: RequestScope::Connection,
            },
            ClientRequest::Initialize(InitializeParams::default()),
        )
        .await
        .expect_err("second initialize fails");
    assert_eq!(duplicate.data.unwrap()["kind"], "already_initialized");

    handler_b
        .handle_json_rpc_request(
            JsonRpcRequestContext {
                connection: connection_b,
                scope: RequestScope::Connection,
            },
            ClientRequest::KeepAlive,
        )
        .await
        .expect("connection B remains usable");
}

#[tokio::test]
async fn local_bridge_restore_session_seq_from_watermark_restores_replay_and_allocator() {
    let state = Arc::new(AppServerHostState::default());
    let bridge = AppServerLocalBridge::new(Arc::clone(&state));
    let session_id =
        coco_types::SessionId::try_new("sess-local-seq-resume").expect("valid test session id");
    let watermark = 40;

    super::super::session_registry::restore_session_seq_from_watermark(
        bridge.app_server(),
        &state,
        session_id.clone(),
        watermark,
    );

    let adapter = coco_app_server::LocalClientAdapter::with_channel_capacity(
        Arc::clone(bridge.app_server()),
        8,
    );
    let stale_connection = adapter.connect();
    let stale_replay = stale_connection
        .subscribe_surface(
            session_id.clone(),
            Some(0),
            coco_app_server::AttachSurfaceOptions::default(),
        )
        .expect("stale subscribe should resolve");
    assert!(matches!(
        stale_replay,
        coco_app_server::LocalClientSubscribeOutcome::SnapshotRequired
    ));

    route_app_server_session_event(
        bridge.app_server(),
        None,
        state.session_seq_allocator(),
        session_id.clone(),
        coco_types::CoreEvent::Protocol(coco_types::ServerNotification::SessionStateChanged {
            state: coco_types::SessionState::Running,
        }),
    );

    let high_water = state
        .session_seq_allocator()
        .high_water(&session_id)
        .expect("durable event should allocate a session_seq");
    assert!(high_water > watermark);
}

#[tokio::test]
async fn local_turn_completion_rejects_terminal_without_session_result() {
    let home = tempfile::TempDir::new().expect("home tempdir");
    let state = Arc::new(AppServerHostState::default());
    let mut bridge = AppServerLocalBridge::new(state);
    let runtime = build_local_bridge_test_runtime(&home).await;
    let session_id = runtime.session_id().clone();
    bridge
        .bind_interactive_session(runtime, None)
        .await
        .expect("bind local session");
    bridge
        .handler()
        .set_turn_runner(Arc::new(NoResultTurnRunner))
        .await;

    let error = bridge
        .start_turn_and_wait_for_end(
            session_id.clone(),
            coco_types::TurnStartParams {
                target: coco_types::InteractiveTarget {
                    session_id,
                    surface_id: coco_types::SurfaceId::generate(),
                },
                prompt: "terminal without result".to_string(),
                history_override: Vec::new(),
                images: Vec::new(),
                slash_metadata: None,
                model_selection: None,
                permission_mode: None,
                thinking_level: None,
                goal_continuation: false,
            },
        )
        .await
        .expect_err("missing terminal session_result must fail");
    assert!(
        error
            .to_string()
            .contains("ended without per-turn session_result"),
        "unexpected error: {error}"
    );
}

async fn build_local_bridge_test_runtime(
    home: &tempfile::TempDir,
) -> crate::session_runtime::SessionHandle {
    let settings = coco_config::SettingsWithSource {
        merged: coco_config::Settings {
            models: coco_config::ModelSelectionSettings {
                main: Some(coco_config::RoleSlots::new(
                    coco_types::ProviderModelSelection {
                        provider: "anthropic".into(),
                        model_id: "claude-opus-4-7".into(),
                    },
                )),
                ..Default::default()
            },
            ..Default::default()
        },
        per_source: std::collections::HashMap::new(),
        source_paths: std::collections::HashMap::new(),
    };
    let runtime_config = coco_config::build_runtime_config_with(
        settings,
        coco_config::EnvSnapshot::default(),
        coco_config::RuntimeOverrides::default(),
        coco_config::CatalogPaths::empty_in(home.path()),
        coco_config::parse_enabled_setting_sources(None),
    )
    .expect("runtime config");
    let model_id = crate::headless::resolve_main_model(&runtime_config).model_id;
    let factory = crate::session_runtime::SessionRuntimeFactory::new(
        crate::session_runtime::SessionRuntimeFactoryOpts {
            cli: Arc::new(crate::AgentHostOptions::default()),
            bootstrap_source:
                crate::session_runtime::SessionRuntimeBootstrapSource::from_prebuilt_bootstrap(
                    crate::session_runtime::SessionRuntimeBootstrap {
                        runtime_config: Arc::new(runtime_config),
                        tools: Arc::new(coco_tool_runtime::ToolRegistry::new()),
                        model_id,
                        system_prompt: "local bridge test".to_string(),
                        permission_mode_availability:
                            coco_types::PermissionModeAvailability::default(),
                        permission_mode: coco_types::PermissionMode::default(),
                        command_registry: Arc::new(tokio::sync::RwLock::new(Arc::new(
                            coco_commands::CommandRegistry::new(),
                        ))),
                        skill_manager: Arc::new(coco_skills::SkillManager::new()),
                        project_services: Arc::new(coco_app_runtime::ProjectServices::load(
                            home.path(),
                            home.path(),
                        )),
                        agent_search_paths:
                            coco_subagent::definition_store::AgentSearchPaths::empty(),
                    },
                ),
            cwd: home.path().to_path_buf(),
            model_runtimes: None,
            session_manager: Arc::new(coco_session::SessionManager::new(
                home.path().join("sessions"),
            )),
            fast_model_spec: None,
            permission_bridge: None,
            process_runtime: coco_app_runtime::ProcessRuntime::global(),
            builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog::interactive(),
            is_non_interactive: false,
        },
    );
    factory
        .build(None, Default::default())
        .await
        .expect("build SessionRuntime")
}
