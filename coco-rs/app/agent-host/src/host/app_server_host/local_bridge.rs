//! Local in-process bridge for AppServer-backed host handlers.
//!
//! This keeps runtime semantics in `coco-agent-host` while allowing
//! `coco-app-server` to own JSON-RPC connection dispatch and routing.

use std::{sync::Arc, time::Duration};

use crate::local_client::{LocalServerClient, LocalSessionClient};
use coco_app_server::{AppServer, ConnectionLimits, LocalClientAdapter};
use coco_app_server_client::ClientError;
use coco_types::SessionId;
use tokio::{sync::mpsc, task::JoinHandle};
use tracing::warn;

use crate::app_session::AppSessionHandle;
use crate::event_hub::ProcessEventHubEgress;

mod session;

use super::AppServerHostState;
pub(crate) use super::config::APP_SERVER_TURN_DRAIN_TIMEOUT;
use super::config::{
    APP_SERVER_LOCAL_CHANNEL_CAPACITY, APP_SERVER_LOCAL_RETENTION_PER_SESSION,
    server_config_duration_secs, server_config_usize,
};
pub use super::handler::AppServerHostHandler;
use super::outbound::{install_session_seq_durability, spawn_app_server_local_outbound_forwarder};
use super::session_close::shutdown_local_app_server_sessions;
use super::session_registry::register_local_app_server_session;

#[derive(Debug, Clone)]
pub struct AppServerLocalTurnCompletion {
    pub started: coco_types::TurnStartResult,
    pub ended: coco_types::TurnEndedParams,
    pub session_result: coco_types::SessionResultParams,
}

#[derive(Clone)]
pub struct AppServerLocalSessionBinding {
    pub session: crate::session_runtime::SessionHandle,
    pub client: LocalSessionClient,
}

pub struct AppServerLocalBridge {
    app_server: Arc<AppServer<AppSessionHandle>>,
    client: LocalServerClient<AppSessionHandle>,
    handler: AppServerHostHandler,
    outbound_forwarder: JoinHandle<()>,
    hub_connector: Arc<std::sync::RwLock<Option<ProcessEventHubEgress>>>,
    event_pump: Option<JoinHandle<()>>,
    event_pump_session_id: Option<SessionId>,
    full_session: Option<LocalSessionClient>,
    // At most one ephemeral sidechat child coexists with the primary session
    // (I-2), so a single extra pair of slots suffices — no map needed. The
    // primary fields above are never disturbed by child lifecycle.
    child_full_session: Option<LocalSessionClient>,
    child_event_pump: Option<JoinHandle<()>>,
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
            server_config_duration_secs(
                server_config.turn_drain_timeout_secs,
                APP_SERVER_TURN_DRAIN_TIMEOUT,
            ),
            ConnectionLimits {
                max_attached_sessions_per_connection: server_config_usize(
                    server_config.max_attached_sessions_per_connection,
                    ConnectionLimits::default().max_attached_sessions_per_connection,
                ),
                max_connections_per_session: server_config_usize(
                    server_config.max_connections_per_session,
                    ConnectionLimits::default().max_connections_per_session,
                ),
            },
            server_config_duration_secs(
                server_config.server_request_timeout_secs,
                Duration::from_secs(15 * 60),
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
            APP_SERVER_TURN_DRAIN_TIMEOUT,
            ConnectionLimits::default(),
            Duration::from_secs(15 * 60),
        )
    }

    fn with_capacity_and_retention(
        state: Arc<AppServerHostState>,
        channel_capacity: usize,
        event_retention_per_session: usize,
        turn_drain_timeout: Duration,
        connection_limits: ConnectionLimits,
        server_request_timeout: Duration,
    ) -> Self {
        assert!(
            channel_capacity > 0,
            "local AppServer bridge channel capacity must be non-zero"
        );
        assert!(
            event_retention_per_session > 0,
            "local AppServer bridge event retention must be non-zero"
        );
        let app_server = Arc::new(
            AppServer::<AppSessionHandle>::with_connection_limits_and_server_request_timeout(
                // Capacity guard: the primary session plus at most one ephemeral
                // sidechat child (I-2). Correctness is enforced by the registry's
                // parent→child index, not this number.
                /*max_sessions*/
                2,
                event_retention_per_session,
                connection_limits,
                server_request_timeout,
            ),
        );
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
            full_session: None,
            child_full_session: None,
            child_event_pump: None,
        }
    }

    pub fn app_server(&self) -> &Arc<AppServer<AppSessionHandle>> {
        &self.app_server
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

    pub fn connect_local_client(&self) -> LocalServerClient<AppSessionHandle> {
        self.client.clone()
    }

    pub fn handler(&self) -> &AppServerHostHandler {
        &self.handler
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
            Arc::clone(&self.handler.state),
            AppSessionHandle::from_runtime(session.clone()),
            self.handler.turn_drain_timeout,
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
        if let Some(handle) = &self.child_event_pump {
            handle.abort();
        }
    }
}

#[cfg(test)]
#[path = "local_bridge.test.rs"]
mod tests;
