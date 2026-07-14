use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::Result;
use coco_app_runtime::ProcessRuntime;
use coco_config::global_config;
use coco_session::SessionManager;
use coco_types::CoreEvent;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::app_server_host::{
    AppServerHostState, HostInputs, SessionTurnExecutor, build_remote_app_server_runtime_binding,
    build_remote_initialize_bootstrap, open_remote_sidecar_binding, spawn_idle_session_sweep,
};
use crate::event_hub::{
    ProcessEventHub, ProcessEventHubEgress, spawn_app_server_membership_watcher,
};
use crate::session_bootstrap::build_engine_resources;

pub use crate::app_server_host::{
    RemoteAppServer, RemoteAppServerBridgeHost, RemoteAppServerConnectionBinding,
    RemoteAppServerHandle, RemoteJsonRpcAdapter, RemoteJsonRpcConnection, RemoteOutboundMessage,
    RemoteSidecarHostBinding,
};

pub struct RemoteHostOptions {
    pub agent_host_options: crate::AgentHostOptions,
    pub max_turns: Option<i32>,
}

pub struct HostBuilder {
    options: RemoteHostOptions,
    cwd: PathBuf,
    process_runtime: Arc<ProcessRuntime>,
}

pub struct PreparedHost {
    runtime_config: coco_config::RuntimeConfig,
    state: Arc<AppServerHostState>,
    app_server: Arc<RemoteAppServer>,
    adapter: RemoteJsonRpcAdapter,
    process_runtime: Arc<ProcessRuntime>,
    app_server_turn_drain_timeout: Duration,
    plugin_notifications: Option<mpsc::Receiver<CoreEvent>>,
    event_hub_connector: Option<ProcessEventHub>,
    _plugin_watcher_guard: Option<Arc<dyn Send + Sync>>,
    _idle_session_sweep: Option<JoinHandle<()>>,
    _event_hub_membership_watcher: Option<JoinHandle<()>>,
}

impl PreparedHost {
    pub fn runtime_config(&self) -> &coco_config::RuntimeConfig {
        &self.runtime_config
    }

    pub fn bridge_host(&self) -> RemoteAppServerBridgeHost {
        RemoteAppServerBridgeHost::new(Arc::clone(&self.state))
            .with_hub_connector_egress(self.hub_connector())
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

    fn hub_connector(&self) -> Option<ProcessEventHubEgress> {
        self.event_hub_connector
            .as_ref()
            .map(ProcessEventHub::egress)
    }

    pub fn take_plugin_notifications(&mut self) -> Option<mpsc::Receiver<CoreEvent>> {
        self.plugin_notifications.take()
    }

    pub fn open_sidecar_binding(&self, channel_capacity: usize) -> RemoteSidecarHostBinding {
        open_remote_sidecar_binding(
            Arc::clone(&self.state),
            Arc::clone(&self.app_server),
            self.hub_connector(),
            self.app_server_turn_drain_timeout,
            channel_capacity,
        )
    }

    pub async fn shutdown(self) -> Result<()> {
        let Self {
            runtime_config,
            state,
            app_server,
            app_server_turn_drain_timeout,
            process_runtime,
            event_hub_connector,
            _event_hub_membership_watcher,
            ..
        } = self;
        let shutdown_timeout =
            Duration::from_secs(runtime_config.server.shutdown_timeout_secs as u64);
        let app_server_shutdown = crate::app_server_host::shutdown_remote_app_server_host(
            app_server,
            state,
            app_server_turn_drain_timeout,
            shutdown_timeout,
        )
        .await;
        let shutdown = crate::shutdown::ShutdownCoordinator::new("remote", shutdown_timeout)
            .finish_after_app_server(
                app_server_shutdown,
                event_hub_connector,
                _event_hub_membership_watcher,
            )
            .await;
        process_runtime.shutdown_background_tasks();
        shutdown.into_result("remote")
    }
}

impl HostBuilder {
    pub fn new(
        options: RemoteHostOptions,
        cwd: PathBuf,
        process_runtime: Arc<ProcessRuntime>,
    ) -> Self {
        Self {
            options,
            cwd,
            process_runtime,
        }
    }

    pub async fn prepare(self) -> Result<PreparedHost> {
        let Self {
            options,
            cwd,
            process_runtime,
        } = self;

        let agent_host_options = Arc::new(options.agent_host_options);
        tracing::info!(
            target: "coco_agent_host::remote",
            cwd = %cwd.display(),
            "remote host starting"
        );
        let runtime_config =
            crate::headless::build_runtime_config_for_cli(&agent_host_options, &cwd)?;
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
        let requires_structured_output = agent_host_options.json_schema.is_some();
        let runner = Arc::new(SessionTurnExecutor::new(options.max_turns, system_prompt));
        let state = Arc::new(AppServerHostState::new(HostInputs {
            startup_cwd: Some(cwd.clone()),
            initialize_bootstrap: Some(bootstrap),
            session_manager: Some(session_manager),
            bypass_permissions_available,
            runtime_replacement: Some(crate::app_server_host::RuntimeReplacementContext {
                runtime_factory,
                process_runtime: process_runtime.clone(),
                cwd: cwd.clone(),
                requires_structured_output,
                integration_options: crate::session_bootstrap::SessionIntegrationOptions::default(),
            }),
            turn_runner: Some(runner),
        }));

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

        let event_hub_connector = ProcessEventHub::spawn(&runtime_config, &cwd, Vec::new());
        let event_hub_membership_watcher = event_hub_connector.as_ref().map(|connector| {
            spawn_app_server_membership_watcher(Arc::clone(&app_server), connector.updater())
        });

        tracing::info!(
            target: "coco_agent_host::remote",
            permission_mode = ?permission_mode,
            bypass_available = bypass_permissions_available,
            "remote host ready"
        );

        Ok(PreparedHost {
            runtime_config,
            state,
            app_server,
            adapter,
            process_runtime,
            app_server_turn_drain_timeout,
            plugin_notifications: Some(plugin_notif_rx),
            event_hub_connector,
            _plugin_watcher_guard: plugin_watcher_guard,
            _idle_session_sweep: idle_session_sweep,
            _event_hub_membership_watcher: event_hub_membership_watcher,
        })
    }
}

#[cfg(test)]
#[path = "remote_host.test.rs"]
mod tests;
