use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::Result;
use coco_app_runtime::ProcessRuntime;
use coco_config::global_config;
use coco_hub_connector::HubConnectorSender;
use coco_session::SessionManager;
use coco_types::CoreEvent;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::app_server_host::{
    AppServerHostState, SessionTurnExecutor, build_remote_app_server_runtime_binding,
    build_remote_initialize_bootstrap, install_app_server_session_runtime_state,
    load_local_app_server_session_runtime, open_remote_sidecar_binding, spawn_idle_session_sweep,
};
use crate::event_hub::RuntimeEventHubConnector;
use crate::session_bootstrap::{
    SessionIntegrationOptions, build_engine_resources, install_session_integrations,
};

pub use crate::app_server_host::{
    RemoteAppServer, RemoteAppServerBridgeHost, RemoteAppServerConnectionBinding,
    RemoteAppServerHandle, RemoteJsonRpcAdapter, RemoteJsonRpcConnection, RemoteOutboundMessage,
    RemoteSidecarHostBinding,
};

pub struct RemoteHostOptions {
    pub agent_host_options: crate::AgentHostOptions,
    pub max_turns: Option<i32>,
}

pub struct PreparedRemoteHost {
    runtime_config: coco_config::RuntimeConfig,
    state: Arc<AppServerHostState>,
    app_server: Arc<RemoteAppServer>,
    adapter: RemoteJsonRpcAdapter,
    app_server_turn_drain_timeout: Duration,
    plugin_notifications: Option<mpsc::Receiver<CoreEvent>>,
    event_hub_connector: Option<RuntimeEventHubConnector>,
    _plugin_watcher_guard: Option<Arc<dyn Send + Sync>>,
    _idle_session_sweep: Option<JoinHandle<()>>,
}

impl PreparedRemoteHost {
    pub fn runtime_config(&self) -> &coco_config::RuntimeConfig {
        &self.runtime_config
    }

    pub fn bridge_host(&self) -> RemoteAppServerBridgeHost {
        RemoteAppServerBridgeHost::new(Arc::clone(&self.state))
            .with_hub_connector_sender(self.hub_sender())
            .with_turn_drain_timeout(self.app_server_turn_drain_timeout)
    }

    pub fn adapter(&self) -> RemoteJsonRpcAdapter {
        self.adapter.clone()
    }

    pub fn connect(&self) -> RemoteJsonRpcConnection {
        self.adapter.connect()
    }

    pub fn shutdown_timeout(&self) -> Duration {
        Duration::from_secs(self.runtime_config.server.shutdown_timeout_secs as u64)
    }

    fn hub_sender(&self) -> Option<HubConnectorSender> {
        self.event_hub_connector
            .as_ref()
            .map(RuntimeEventHubConnector::sender)
    }

    pub fn take_plugin_notifications(&mut self) -> Option<mpsc::Receiver<CoreEvent>> {
        self.plugin_notifications.take()
    }

    pub fn open_sidecar_binding(&self, channel_capacity: usize) -> RemoteSidecarHostBinding {
        open_remote_sidecar_binding(
            Arc::clone(&self.state),
            Arc::clone(&self.app_server),
            self.hub_sender(),
            self.app_server_turn_drain_timeout,
            channel_capacity,
        )
    }

    pub async fn shutdown(self) -> Result<()> {
        let shutdown_timeout = self.shutdown_timeout();
        crate::app_server_host::shutdown_remote_app_server_host(
            self.app_server,
            self.state,
            self.event_hub_connector,
            self.app_server_turn_drain_timeout,
            shutdown_timeout,
        )
        .await
    }
}

pub async fn prepare_remote_host(
    options: RemoteHostOptions,
    cwd: PathBuf,
    process_runtime: Arc<ProcessRuntime>,
) -> Result<PreparedRemoteHost> {
    let agent_host_options = Arc::new(options.agent_host_options);
    tracing::info!(
        target: "coco_agent_host::remote",
        cwd = %cwd.display(),
        "remote host starting"
    );
    let runtime_config = crate::headless::build_runtime_config_for_cli(&agent_host_options, &cwd)?;
    crate::model_card_refresh::spawn_if_enabled(&runtime_config);

    let resources =
        build_engine_resources(&process_runtime, &agent_host_options, &runtime_config, &cwd)?;
    let system_prompt = Some(resources.system_prompt.clone());

    let session_manager = Arc::new(SessionManager::with_backend(
        runtime_config.settings.merged.session.backend,
        global_config::config_home(),
    ));
    let session_manager_for_runtime = session_manager.clone();
    let bootstrap = build_remote_initialize_bootstrap(&resources, &runtime_config, &cwd).await;

    if let Some(msg) = &resources.startup.notification {
        eprintln!("warning: {msg}");
    }
    let bypass_permissions_available = resources.startup.bypass_available;
    let permission_mode = resources.startup.mode;

    let (plugin_notif_tx, plugin_notif_rx) = mpsc::channel(16);
    let plugin_watcher_guard =
        crate::plugin_watch::spawn(plugin_notif_tx, &cwd, &global_config::config_home())
            .map(|guard| guard as Arc<dyn Send + Sync>);
    crate::session_bootstrap::spawn_marketplace_startup(global_config::config_home());
    let state = Arc::new(AppServerHostState::default());
    state.install_startup_cwd(cwd.clone());
    state.install_session_manager_for_startup(session_manager);
    state.install_initialize_bootstrap_for_startup(bootstrap);
    state.set_bypass_permissions_available(bypass_permissions_available);

    let app_server_binding =
        build_remote_app_server_runtime_binding(&state, &runtime_config.server);
    let app_server = app_server_binding.app_server;
    let adapter = app_server_binding.adapter;
    let app_server_turn_drain_timeout = app_server_binding.turn_drain_timeout;

    let idle_session_sweep = runtime_config
        .server
        .idle_session_timeout_secs
        .filter(|secs| *secs > 0)
        .map(|secs| {
            spawn_idle_session_sweep(
                Arc::clone(&app_server),
                Arc::clone(&state),
                Duration::from_secs(secs as u64),
                app_server_turn_drain_timeout,
            )
        });

    let runtime_factory = crate::session_runtime::SessionRuntimeFactory::from_host_config(
        crate::session_runtime::SessionRuntimeFactoryHostConfig {
            cli: Arc::clone(&agent_host_options),
            cwd: cwd.clone(),
            model_runtimes: None,
            session_manager: session_manager_for_runtime,
            fast_model_spec: None,
            permission_bridge: None,
            process_runtime: process_runtime.clone(),
            builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog::interactive(),
            is_non_interactive: true,
        },
    );
    let startup_session_id = coco_types::SessionId::generate();
    let runtime_replacement_factory = runtime_factory.clone();
    let loaded_handle = load_local_app_server_session_runtime(
        &app_server,
        startup_session_id.clone(),
        runtime_factory,
    )
    .await
    .map_err(|error| anyhow::anyhow!("{}", error.message))?;
    let session_handle = loaded_handle.into_session();
    let mcp_manager = Arc::new(tokio::sync::Mutex::new(
        coco_mcp::McpConnectionManager::new_with_runtime_config(
            global_config::config_home(),
            &session_handle.runtime_config().mcp,
        ),
    ));
    let event_hub_connector = {
        let session_id = session_handle.session_id().clone();
        RuntimeEventHubConnector::spawn_for_session(
            session_handle.runtime_config(),
            session_id,
            &cwd,
        )
    };

    let requires_structured_output = session_handle
        .install_structured_output_tool_if_requested(agent_host_options.json_schema.as_deref())
        .await?;

    install_session_integrations(
        session_handle.clone(),
        &cwd,
        process_runtime.clone(),
        SessionIntegrationOptions {
            existing_mcp_manager: Some(mcp_manager),
            ..Default::default()
        },
    )
    .await?;

    session_handle
        .fire_session_start_hooks(coco_hooks::orchestration::SessionStartSource::Startup)
        .await;
    session_handle
        .fire_setup_hooks(coco_hooks::orchestration::SetupTrigger::Maintenance)
        .await;

    install_app_server_session_runtime_state(
        state.clone(),
        session_handle.clone(),
        Arc::clone(&app_server),
    )
    .await;

    state
        .install_runtime_replacement(crate::app_server_host::RuntimeReplacementContext {
            startup_session_id,
            runtime_factory: runtime_replacement_factory,
            process_runtime: process_runtime.clone(),
            cwd: cwd.clone(),
            requires_structured_output,
        })
        .await;
    let runner = Arc::new(SessionTurnExecutor::new(options.max_turns, system_prompt));
    state.install_turn_runner(runner).await;

    tracing::info!(
        target: "coco_agent_host::remote",
        permission_mode = ?permission_mode,
        bypass_available = bypass_permissions_available,
        "remote host ready"
    );

    Ok(PreparedRemoteHost {
        runtime_config,
        state,
        app_server,
        adapter,
        app_server_turn_drain_timeout,
        plugin_notifications: Some(plugin_notif_rx),
        event_hub_connector,
        _plugin_watcher_guard: plugin_watcher_guard,
        _idle_session_sweep: idle_session_sweep,
    })
}
