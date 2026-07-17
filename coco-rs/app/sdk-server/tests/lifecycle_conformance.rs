#![allow(clippy::expect_used)]

use std::sync::Arc;

use coco_agent_host::{
    AgentHostOptions,
    app_server_host::{
        AppServerHostState, HostInputs, RemoteAppServer, RemoteAppServerBridgeHost,
        RemoteJsonRpcAdapter, RuntimeReplacementContext,
    },
    session_runtime::{
        SessionRuntimeBootstrap, SessionRuntimeBootstrapSource, SessionRuntimeFactory,
        SessionRuntimeFactoryOpts,
    },
};
use coco_app_server_transport::{JsonRpcFrame, JsonRpcId, JsonRpcNotification, JsonRpcRequest};
use coco_sdk_server::{InMemoryTransport, SdkServer, SdkTransport};
use coco_types::{
    ClientRequestMethod, InitializeParams, InteractiveTarget, SessionCloseParams,
    SessionCloseTarget, SessionReadParams, SessionReadResult, SessionResumeParams,
    SessionResumeResult, SessionStartParams, SessionStartResult, SessionTarget,
};

struct Fixture {
    _home: tempfile::TempDir,
    server: Arc<RemoteAppServer>,
    client: Arc<InMemoryTransport>,
    server_task: tokio::task::JoinHandle<
        Result<coco_app_server::DisconnectOutcome, coco_sdk_server::RemoteAppServerBridgeError>,
    >,
}

struct IsolatedTestBootstrapSource {
    runtime_config: Arc<coco_config::RuntimeConfig>,
    project_services: Arc<coco_app_runtime::ProjectServices>,
}

impl coco_app_runtime::BootstrapSource for IsolatedTestBootstrapSource {
    fn bootstrap_for_session(
        &self,
        _cwd: &std::path::Path,
        _session_id_override: Option<&coco_types::SessionId>,
    ) -> Result<coco_app_runtime::SessionRuntimeBootstrapBuild, coco_app_runtime::BootstrapError>
    {
        Ok(coco_app_runtime::SessionRuntimeBootstrapBuild {
            bootstrap: Arc::new(SessionRuntimeBootstrap {
                runtime_config: Arc::clone(&self.runtime_config),
                tools: Arc::new(coco_tool_runtime::ToolRegistry::new()),
                model_id: "claude-opus-4-7".to_string(),
                system_prompt: "sdk lifecycle conformance test".to_string(),
                permission_mode_availability: coco_types::PermissionModeAvailability::default(),
                permission_mode: coco_types::PermissionMode::default(),
                command_registry: Arc::new(tokio::sync::RwLock::new(Arc::new(
                    coco_commands::CommandRegistry::new(),
                ))),
                skill_manager: Arc::new(coco_skills::SkillManager::new()),
                project_services: Arc::clone(&self.project_services),
                agent_search_paths: coco_subagent::definition_store::AgentSearchPaths::empty(),
            }),
            config_reloader: None,
        })
    }
}

async fn fixture() -> Fixture {
    let home = tempfile::TempDir::new().expect("fixture home");
    let settings = coco_config::SettingsWithSource {
        merged: coco_config::Settings {
            file_checkpointing_enabled: true,
            models: coco_config::ModelSelectionSettings {
                main: Some(coco_config::RoleSlots::new(
                    coco_types::ProviderModelSelection {
                        provider: "anthropic".to_string(),
                        model_id: "claude-opus-4-7".to_string(),
                    },
                )),
                ..Default::default()
            },
            ..Default::default()
        },
        per_source: std::collections::HashMap::new(),
        source_paths: std::collections::HashMap::new(),
    };
    let runtime_config = Arc::new(
        coco_config::build_runtime_config_with(
            settings,
            coco_config::EnvSnapshot::default(),
            coco_config::RuntimeOverrides::default(),
            coco_config::CatalogPaths::empty_in(home.path()),
            coco_config::parse_enabled_setting_sources(None),
        )
        .expect("runtime config"),
    );
    let process_runtime = coco_app_runtime::ProcessRuntime::global();
    let project_services = Arc::new(coco_app_runtime::ProjectServices::load(
        home.path(),
        home.path(),
    ));
    let runtime_factory = SessionRuntimeFactory::new(SessionRuntimeFactoryOpts {
        cli: Arc::new(AgentHostOptions::default()),
        bootstrap_source: SessionRuntimeBootstrapSource::from_source(Arc::new(
            IsolatedTestBootstrapSource {
                runtime_config: Arc::clone(&runtime_config),
                project_services,
            },
        )),
        cwd: home.path().to_path_buf(),
        model_runtimes: None,
        session_manager: Arc::new(coco_session::SessionManager::new(
            home.path().join("sessions"),
        )),
        fast_model_spec: None,
        permission_bridge: None,
        process_runtime: Arc::clone(&process_runtime),
        builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog::noninteractive(),
        is_non_interactive: false,
    });
    let state = Arc::new(AppServerHostState::new(HostInputs {
        startup_cwd: Some(home.path().to_path_buf()),
        runtime_replacement: Some(RuntimeReplacementContext {
            runtime_factory,
            process_runtime,
            cwd: home.path().to_path_buf(),
            requires_structured_output: false,
            integration_options:
                coco_agent_host::session_bootstrap::SessionIntegrationOptions::default(),
        }),
        ..Default::default()
    }));
    let server = Arc::new(RemoteAppServer::new(8, 16));
    let adapter = RemoteJsonRpcAdapter::with_channel_capacity(Arc::clone(&server), 16);
    let (server_end, client) = InMemoryTransport::pair(32);
    let bridge_host = RemoteAppServerBridgeHost::new(Arc::clone(&state));
    let sdk_server = SdkServer::new(server_end, bridge_host);
    let connection = adapter.connect();
    let server_task =
        tokio::spawn(async move { sdk_server.run_app_server_connection(connection).await });

    Fixture {
        _home: home,
        server,
        client,
        server_task,
    }
}

impl Fixture {
    async fn shutdown(self) {
        drop(self.client);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), self.server_task).await;
    }
}

#[tokio::test]
async fn sdk_stdio_shares_start_read_close_lifecycle_contract() {
    let fixture = fixture().await;
    let mut next_request_id = 1;
    let mut notifications = Vec::new();

    json_rpc_success::<serde_json::Value>(
        request(
            &fixture.client,
            &mut next_request_id,
            &mut notifications,
            ClientRequestMethod::Initialize,
            InitializeParams::default(),
            "sdk stdio initialize",
        )
        .await,
        "sdk stdio initialize",
    );

    let resume_seed_text = "resumed-through-sdk-stdio";
    let started: SessionStartResult = json_rpc_success(
        request(
            &fixture.client,
            &mut next_request_id,
            &mut notifications,
            ClientRequestMethod::SessionStart,
            SessionStartParams::default(),
            "sdk stdio session/start",
        )
        .await,
        "sdk stdio session/start",
    );
    let surface_id = started.surface_id.clone();
    let live = fixture.server.list_live_sessions();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].session_id, started.session_id);
    assert_eq!(live[0].surface_counts.attached, 1);

    let read: SessionReadResult = json_rpc_success(
        request(
            &fixture.client,
            &mut next_request_id,
            &mut notifications,
            ClientRequestMethod::SessionRead,
            SessionReadParams {
                target: SessionTarget {
                    session_id: started.session_id.clone(),
                },
                cursor: None,
                limit: None,
            },
            "sdk stdio session/read",
        )
        .await,
        "sdk stdio session/read",
    );
    assert_eq!(read.session.session_id, started.session_id);
    append_durable_transcript_seed(&fixture, &started.session_id, resume_seed_text);

    json_rpc_success::<()>(
        request(
            &fixture.client,
            &mut next_request_id,
            &mut notifications,
            ClientRequestMethod::SessionClose,
            SessionCloseParams {
                target: SessionCloseTarget::Interactive {
                    target: InteractiveTarget {
                        session_id: started.session_id.clone(),
                        surface_id,
                    },
                },
            },
            "sdk stdio session/close",
        )
        .await,
        "sdk stdio session/close",
    );
    assert!(fixture.server.list_live_sessions().is_empty());
    assert_eq!(
        wait_for_session_result_stop_reason(
            &fixture.client,
            &mut notifications,
            &started.session_id,
        )
        .await
        .as_deref(),
        Some("closed")
    );

    let resumed: SessionResumeResult = json_rpc_success(
        request(
            &fixture.client,
            &mut next_request_id,
            &mut notifications,
            ClientRequestMethod::SessionResume,
            SessionResumeParams {
                target: SessionTarget {
                    session_id: started.session_id.clone(),
                },
                plan_mode_instructions: None,
            },
            "sdk stdio session/resume",
        )
        .await,
        "sdk stdio session/resume",
    );
    assert_eq!(resumed.session.session_id, started.session_id);
    let resumed_surface_id = resumed.surface_id.clone();
    let resumed_read: SessionReadResult = json_rpc_success(
        request(
            &fixture.client,
            &mut next_request_id,
            &mut notifications,
            ClientRequestMethod::SessionRead,
            SessionReadParams {
                target: SessionTarget {
                    session_id: started.session_id.clone(),
                },
                cursor: None,
                limit: None,
            },
            "sdk stdio resumed session/read",
        )
        .await,
        "sdk stdio resumed session/read",
    );
    assert!(
        resumed_read
            .messages
            .iter()
            .any(|message| message.to_string().contains(resume_seed_text))
    );
    json_rpc_success::<()>(
        request(
            &fixture.client,
            &mut next_request_id,
            &mut notifications,
            ClientRequestMethod::SessionClose,
            SessionCloseParams {
                target: SessionCloseTarget::Interactive {
                    target: InteractiveTarget {
                        session_id: started.session_id.clone(),
                        surface_id: resumed_surface_id,
                    },
                },
            },
            "sdk stdio resumed session/close",
        )
        .await,
        "sdk stdio resumed session/close",
    );

    fixture.shutdown().await;
}

fn append_durable_transcript_seed(
    fixture: &Fixture,
    session_id: &coco_types::SessionId,
    seed_text: &str,
) {
    let runtime = fixture
        .server
        .registry()
        .get(session_id)
        .expect("live runtime for transcript seed")
        .into_session();
    runtime
        .session_manager_handle()
        .store_for(runtime.original_cwd())
        .append_message(
            session_id.as_str(),
            &coco_session::storage::TranscriptEntry {
                entry_type: "user".to_string(),
                uuid: format!("{session_id}-resume-seed"),
                parent_uuid: None,
                logical_parent_uuid: None,
                session_id: Some(session_id.clone()),
                cwd: runtime.original_cwd().to_string_lossy().into_owned(),
                timestamp: "2026-07-13T00:00:00Z".to_string(),
                version: None,
                git_branch: None,
                is_sidechain: false,
                agent_id: None,
                message: Some(serde_json::json!({"role": "user", "content": seed_text})),
                usage: None,
                model: None,
                request_id: None,
                cost_usd: None,
                extra: serde_json::Map::new(),
            },
        )
        .expect("seed durable transcript");
}

async fn request<T: serde::Serialize>(
    transport: &InMemoryTransport,
    next_request_id: &mut i64,
    notifications: &mut Vec<JsonRpcNotification>,
    method: ClientRequestMethod,
    params: T,
    context: &str,
) -> JsonRpcFrame {
    let request = json_rpc_request(next_request_id, method, params);
    let expected_id = request.id.clone();
    transport
        .send_frame(JsonRpcFrame::Request(request))
        .await
        .expect("send SDK transport request");
    loop {
        let frame = transport
            .recv_frame()
            .await
            .expect("receive SDK transport frame")
            .expect("SDK transport returned EOF");
        match frame {
            JsonRpcFrame::Success(success) if success.id == expected_id => {
                return JsonRpcFrame::Success(success);
            }
            JsonRpcFrame::Error(error) if error.id == expected_id => {
                return JsonRpcFrame::Error(error);
            }
            JsonRpcFrame::Notification(notification) => notifications.push(notification),
            other => panic!("{context} received unexpected frame: {other:?}"),
        }
    }
}

fn json_rpc_request<T: serde::Serialize>(
    next_request_id: &mut i64,
    method: ClientRequestMethod,
    params: T,
) -> JsonRpcRequest {
    let id = *next_request_id;
    *next_request_id += 1;
    JsonRpcRequest::new(
        JsonRpcId::Number(id),
        method.as_str(),
        Some(serde_json::to_value(params).expect("serialize JSON-RPC params")),
    )
}

fn json_rpc_success<T: serde::de::DeserializeOwned>(frame: JsonRpcFrame, context: &str) -> T {
    match frame {
        JsonRpcFrame::Success(success) => {
            serde_json::from_value(success.result).expect("decode JSON-RPC success result")
        }
        JsonRpcFrame::Error(error) => panic!("{context} failed: {:?}", error.error),
        other => panic!("{context} returned unexpected frame: {other:?}"),
    }
}

async fn wait_for_session_result_stop_reason(
    transport: &InMemoryTransport,
    notifications: &mut Vec<JsonRpcNotification>,
    session_id: &coco_types::SessionId,
) -> Option<String> {
    loop {
        if let Some(stop_reason) = notifications.iter().find_map(|notification| {
            notification_session_result_stop_reason(notification, session_id)
        }) {
            return Some(stop_reason);
        }
        let frame = transport
            .recv_frame()
            .await
            .expect("receive SDK transport frame")
            .expect("SDK transport returned EOF");
        if let JsonRpcFrame::Notification(notification) = frame {
            notifications.push(notification);
        }
    }
}

fn notification_session_result_stop_reason(
    notification: &JsonRpcNotification,
    session_id: &coco_types::SessionId,
) -> Option<String> {
    match notification.method.as_str() {
        "session/result" => {
            let params = notification.params.as_ref()?;
            (params.get("session_id")? == session_id.as_str())
                .then(|| params.get("stop_reason")?.as_str().map(str::to_string))?
        }
        "session/event" => {
            let envelope = notification.params.as_ref()?.get("envelope")?;
            (envelope.get("session_id")? == session_id.as_str()).then(|| {
                let event = envelope.get("event")?;
                (event.get("layer")?.as_str()? == "protocol").then_some(())?;
                let payload = event.get("payload")?;
                (payload.get("method")?.as_str()? == "session/result").then_some(())?;
                payload
                    .get("params")?
                    .get("stop_reason")?
                    .as_str()
                    .map(str::to_string)
            })?
        }
        _ => None,
    }
}
