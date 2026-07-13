//! Local in-process bridge for AppServer-backed host handlers.
//!
//! This keeps runtime semantics in `coco-agent-host` while allowing
//! `coco-app-server` to own JSON-RPC connection dispatch and routing.

use std::{future::Future, path::PathBuf, sync::Arc, time::Duration};

use crate::local_client::{LocalServerClient, LocalSessionClient};
use coco_app_server::{AppServer, LocalClientAdapter, SurfaceLimits};
use coco_app_server_client::ClientError;
use coco_types::{SessionId, SurfaceId};
use tokio::{sync::mpsc, task::JoinHandle};
use tracing::warn;

use crate::app_session::AppSessionHandle;
use crate::event_hub::ProcessEventHubEgress;

mod local_interactive_session;

use super::AppServerHostState;
pub(crate) use super::config::APP_SERVER_TURN_DRAIN_TIMEOUT;
use super::config::{
    APP_SERVER_LOCAL_CHANNEL_CAPACITY, APP_SERVER_LOCAL_RETENTION_PER_SESSION,
    server_config_duration_secs, server_config_surface_limits, server_config_usize,
};
pub use super::handler::AppServerHostHandler;
#[cfg(test)]
use super::outbound::route_app_server_session_event;
use super::outbound::{install_session_seq_durability, spawn_app_server_local_outbound_forwarder};
use super::session_close::{close_local_app_server_session, shutdown_local_app_server_sessions};
use super::session_loading::{
    load_local_app_server_session_runtime, load_local_app_server_session_runtime_with_cwd,
    load_local_app_server_session_with_factory,
};
use super::session_registry::{
    register_local_app_server_session, replace_detached_local_app_server_session_with_factory,
    replace_local_app_server_session_with_factory, restore_session_seq_from_watermark,
};

#[derive(Debug, Clone)]
pub struct AppServerLocalTurnCompletion {
    pub started: coco_types::TurnStartResult,
    pub ended: coco_types::TurnEndedParams,
    pub session_result: coco_types::SessionResultParams,
}

#[derive(Clone)]
pub struct AppServerLocalSessionBinding {
    pub session: crate::session_runtime::SessionHandle,
    pub surface: LocalSessionClient,
}

pub struct AppServerLocalBridge {
    app_server: Arc<AppServer<AppSessionHandle>>,
    client: LocalServerClient<AppSessionHandle>,
    handler: AppServerHostHandler,
    outbound_forwarder: JoinHandle<()>,
    hub_connector: Arc<std::sync::RwLock<Option<ProcessEventHubEgress>>>,
    event_pump: Option<JoinHandle<()>>,
    event_pump_session_id: Option<SessionId>,
    interactive_surface: Option<LocalSessionClient>,
    channel_capacity: usize,
}

impl AppServerLocalBridge {
    pub fn new(state: Arc<AppServerHostState>) -> Self {
        Self::with_channel_capacity(state, APP_SERVER_LOCAL_CHANNEL_CAPACITY)
    }

    pub fn with_server_config(
        state: Arc<AppServerHostState>,
        server_config: &coco_config::ServerConfig,
    ) -> Self {
        Self::with_capacity_and_retention(
            state,
            server_config_usize(
                server_config.outbound_queue_frames,
                APP_SERVER_LOCAL_CHANNEL_CAPACITY,
            ),
            server_config_usize(
                server_config.event_retention_per_session,
                APP_SERVER_LOCAL_RETENTION_PER_SESSION,
            ),
            server_config_surface_limits(server_config),
            server_config_duration_secs(
                server_config.turn_drain_timeout_secs,
                APP_SERVER_TURN_DRAIN_TIMEOUT,
            ),
        )
    }

    pub fn with_host_inputs_and_server_config(
        inputs: super::HostInputs,
        server_config: &coco_config::ServerConfig,
    ) -> Self {
        Self::with_server_config(Arc::new(AppServerHostState::new(inputs)), server_config)
    }

    pub fn with_channel_capacity(state: Arc<AppServerHostState>, channel_capacity: usize) -> Self {
        Self::with_capacity_and_retention(
            state,
            channel_capacity,
            channel_capacity,
            SurfaceLimits::default(),
            APP_SERVER_TURN_DRAIN_TIMEOUT,
        )
    }

    fn with_capacity_and_retention(
        state: Arc<AppServerHostState>,
        channel_capacity: usize,
        event_retention_per_session: usize,
        surface_limits: SurfaceLimits,
        turn_drain_timeout: Duration,
    ) -> Self {
        assert!(
            channel_capacity > 0,
            "local AppServer bridge channel capacity must be non-zero"
        );
        assert!(
            event_retention_per_session > 0,
            "local AppServer bridge event retention must be non-zero"
        );
        let app_server = Arc::new(AppServer::<AppSessionHandle>::new_with_surface_limits(
            /*max_sessions*/ 1,
            event_retention_per_session,
            surface_limits,
        ));
        install_session_seq_durability(&state, event_retention_per_session as i64);
        let adapter =
            LocalClientAdapter::with_channel_capacity(Arc::clone(&app_server), channel_capacity);
        let client = LocalServerClient::connect_local(&adapter);
        let (outbound_tx, outbound_rx) = mpsc::channel(channel_capacity);
        let handler = AppServerHostHandler::with_local_app_server_and_turn_drain_timeout(
            Arc::clone(&state),
            outbound_tx,
            Arc::clone(&app_server),
            turn_drain_timeout,
        );
        let hub_connector = Arc::new(std::sync::RwLock::new(None));
        let outbound_forwarder = spawn_app_server_local_outbound_forwarder(
            Arc::clone(&app_server),
            state,
            outbound_rx,
            Arc::clone(&hub_connector),
        );
        Self {
            app_server,
            client,
            handler,
            outbound_forwarder,
            hub_connector,
            event_pump: None,
            event_pump_session_id: None,
            interactive_surface: None,
            channel_capacity,
        }
    }

    pub fn app_server(&self) -> &Arc<AppServer<AppSessionHandle>> {
        &self.app_server
    }

    /// Restore both AppServer replay and durable seq allocation state from a
    /// resumed transcript watermark before the next durable event is emitted.
    pub fn restore_session_seq_from_watermark(&self, session_id: SessionId, watermark: i64) {
        restore_session_seq_from_watermark(
            &self.app_server,
            &self.handler.state,
            session_id,
            watermark,
        );
    }

    pub fn client(&self) -> &LocalServerClient<AppSessionHandle> {
        &self.client
    }

    pub fn set_hub_connector_egress(&self, egress: ProcessEventHubEgress) {
        match self.hub_connector.write() {
            Ok(mut guard) => *guard = Some(egress),
            Err(poisoned) => *poisoned.into_inner() = Some(egress),
        }
    }

    pub fn client_mut(&mut self) -> &mut LocalServerClient<AppSessionHandle> {
        &mut self.client
    }

    pub fn connect_local_client(&self) -> LocalServerClient<AppSessionHandle> {
        let adapter = LocalClientAdapter::with_channel_capacity(
            Arc::clone(&self.app_server),
            self.channel_capacity,
        );
        LocalServerClient::connect_local(&adapter)
    }

    pub fn handler(&self) -> &AppServerHostHandler {
        &self.handler
    }

    pub async fn close_registered_session(&self, session_id: SessionId) -> Result<(), ClientError> {
        close_local_app_server_session(
            Arc::clone(&self.app_server),
            Arc::clone(&self.handler.state),
            session_id,
            self.handler.turn_drain_timeout,
        )
        .await
        .map_err(|error| ClientError::Server {
            code: error.code,
            message: error.message,
            data: error.data,
        })
    }

    pub async fn shutdown_registered_sessions(&self) -> Result<(), ClientError> {
        shutdown_local_app_server_sessions(
            Arc::clone(&self.app_server),
            Arc::clone(&self.handler.state),
            self.handler.turn_drain_timeout,
        )
        .await
        .map_err(|error| ClientError::Server {
            code: error.code,
            message: error.message,
            data: error.data,
        })
    }

    pub async fn load_session_runtime<F>(
        &self,
        session_id: SessionId,
        factory: F,
    ) -> anyhow::Result<crate::session_runtime::SessionHandle>
    where
        F: Future<Output = anyhow::Result<crate::session_runtime::SessionHandle>> + Send + 'static,
    {
        let handle = load_local_app_server_session_with_factory(
            &self.app_server,
            session_id.clone(),
            async move {
                let runtime = factory.await.map_err(|error| {
                    coco_app_server::RegistryError::load_failed(error.to_string())
                })?;
                Ok::<AppSessionHandle, coco_app_server::RegistryError>(
                    AppSessionHandle::from_runtime(runtime),
                )
            },
        )
        .await
        .map_err(|error| anyhow::anyhow!("{}", error.message))?;
        Ok(handle.into_session())
    }

    pub async fn load_session_runtime_from_factory(
        &self,
        session_id: SessionId,
        runtime_factory: crate::session_runtime::SessionRuntimeFactory,
    ) -> anyhow::Result<crate::session_runtime::SessionHandle> {
        let handle =
            load_local_app_server_session_runtime(&self.app_server, session_id, runtime_factory)
                .await
                .map_err(|error| anyhow::anyhow!("{}", error.message))?;
        Ok(handle.into_session())
    }

    pub async fn load_session_runtime_from_factory_with_cwd(
        &self,
        session_id: SessionId,
        runtime_factory: crate::session_runtime::SessionRuntimeFactory,
        cwd: PathBuf,
    ) -> anyhow::Result<crate::session_runtime::SessionHandle> {
        let handle = load_local_app_server_session_runtime_with_cwd(
            &self.app_server,
            session_id,
            runtime_factory,
            cwd,
        )
        .await
        .map_err(|error| anyhow::anyhow!("{}", error.message))?;
        Ok(handle.into_session())
    }

    pub async fn replace_session_runtime<F>(
        &self,
        old_session_id: SessionId,
        new_session_id: SessionId,
        factory: F,
    ) -> anyhow::Result<Option<(crate::session_runtime::SessionHandle, SurfaceId)>>
    where
        F: Future<Output = anyhow::Result<crate::session_runtime::SessionHandle>> + Send + 'static,
    {
        replace_local_app_server_session_with_factory(
            Arc::clone(&self.app_server),
            Arc::clone(&self.handler.state),
            old_session_id,
            new_session_id,
            async move {
                let runtime = factory.await.map_err(|error| {
                    coco_app_server::RegistryError::load_failed(error.to_string())
                })?;
                Ok(AppSessionHandle::from_runtime(runtime))
            },
            self.handler.turn_drain_timeout,
        )
        .await
        .map(|replacement| {
            replacement.map(|(handle, surface_id)| (handle.into_session(), surface_id))
        })
        .map_err(|error| anyhow::anyhow!("{}", error.message))
    }

    pub async fn replace_detached_session_runtime<F>(
        &self,
        old_session_id: SessionId,
        new_session_id: SessionId,
        factory: F,
    ) -> anyhow::Result<crate::session_runtime::SessionHandle>
    where
        F: Future<Output = anyhow::Result<crate::session_runtime::SessionHandle>> + Send + 'static,
    {
        replace_detached_local_app_server_session_with_factory(
            Arc::clone(&self.app_server),
            Arc::clone(&self.handler.state),
            old_session_id,
            new_session_id,
            async move {
                let runtime = factory.await.map_err(|error| {
                    coco_app_server::RegistryError::load_failed(error.to_string())
                })?;
                Ok(AppSessionHandle::from_runtime(runtime))
            },
            self.handler.turn_drain_timeout,
        )
        .await
        .map(AppSessionHandle::into_session)
        .map_err(|error| anyhow::anyhow!("{}", error.message))
    }

    pub async fn register_session_runtime(&self, session: crate::session_runtime::SessionHandle) {
        crate::app_server_host::hook_callback_bridge::install_runtime_callback(
            Arc::clone(&self.app_server),
            &session,
        );
        let (session_id, bypass_permissions_available, session_manager) = {
            let session_id = session.session_id().clone();
            (
                session_id,
                session.bypass_permissions_available().await,
                session.session_manager_handle(),
            )
        };
        if let Err(error) = register_local_app_server_session(
            &self.app_server,
            AppSessionHandle::from_runtime(session.clone()),
        )
        .await
        {
            warn!(?error, session_id = %session_id, "local AppServer registry install failed");
        }

        session.reset_session_accounting();
        self.handler
            .state
            .touch_session_activity(session_id.clone());
        self.handler
            .state
            .set_bypass_permissions_available(bypass_permissions_available);
        self.handler
            .state
            .install_turn_runner(Arc::new(super::SessionTurnExecutor::new(None, None)))
            .await;
        self.handler
            .state
            .install_session_manager(session_manager)
            .await;
    }
}

impl Drop for AppServerLocalBridge {
    fn drop(&mut self) {
        self.outbound_forwarder.abort();
        if let Some(handle) = &self.event_pump {
            handle.abort();
        }
    }
}

#[cfg(test)]
#[path = "local_bridge.test.rs"]
mod tests;
