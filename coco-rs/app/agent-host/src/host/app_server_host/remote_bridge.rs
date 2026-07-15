use std::sync::Arc;
use std::time::Duration;

use coco_app_server::{AppServer, JsonRpcAdapter};
use coco_types::CoreEvent;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::config::{
    server_config_duration_secs, server_config_surface_limits, server_config_usize,
};
use super::{
    AppServerHostHandler, AppServerHostState, OutboundMessage, ProcessEvent,
    install_session_seq_durability, shutdown_local_app_server_sessions,
    spawn_app_server_local_outbound_forwarder,
};
use crate::app_session::AppSessionHandle;
use crate::event_hub::ProcessEventHubEgress;

const REMOTE_APP_SERVER_MAX_SESSIONS: usize = 32;
const REMOTE_APP_SERVER_RETENTION_PER_SESSION: usize = 1024;
const REMOTE_APP_SERVER_OUTBOUND_QUEUE_FRAMES: usize = 256;

pub type RemoteAppServerHandle = AppSessionHandle;
pub type RemoteAppServer = AppServer<RemoteAppServerHandle>;
pub type RemoteJsonRpcAdapter = JsonRpcAdapter<RemoteAppServerHandle>;
pub type RemoteJsonRpcConnection = coco_app_server::JsonRpcAdapterConnection<RemoteAppServerHandle>;
pub type RemoteOutboundMessage = OutboundMessage;

pub(crate) struct RemoteAppServerRuntimeBinding {
    pub app_server: Arc<RemoteAppServer>,
    pub adapter: RemoteJsonRpcAdapter,
    pub turn_drain_timeout: Duration,
}

pub struct RemoteSidecarHostBinding {
    pub handler: Arc<AppServerHostHandler>,
    pub outbound_forwarder: JoinHandle<()>,
}

pub struct RemoteAppServerConnectionBinding {
    pub handler: Arc<AppServerHostHandler>,
    pub outbound_tx: mpsc::Sender<RemoteOutboundMessage>,
    pub outbound_rx: mpsc::Receiver<RemoteOutboundMessage>,
    pub external_forwarders: Vec<JoinHandle<()>>,
}

#[derive(Clone)]
pub struct RemoteAppServerBridgeHost {
    state: Arc<AppServerHostState>,
    hub_connector: Option<ProcessEventHubEgress>,
    turn_drain_timeout: Duration,
}

impl RemoteAppServerBridgeHost {
    pub fn new(state: Arc<AppServerHostState>) -> Self {
        Self {
            state,
            hub_connector: None,
            turn_drain_timeout: super::APP_SERVER_TURN_DRAIN_TIMEOUT,
        }
    }

    pub fn ephemeral() -> Self {
        Self::new(Arc::new(AppServerHostState::default()))
    }

    pub fn with_hub_connector_egress(mut self, egress: Option<ProcessEventHubEgress>) -> Self {
        self.hub_connector = egress;
        self
    }

    pub fn with_turn_drain_timeout(mut self, timeout: Duration) -> Self {
        self.turn_drain_timeout = timeout;
        self
    }

    pub fn turn_drain_timeout(&self) -> Duration {
        self.turn_drain_timeout
    }

    pub fn session_seq_allocator(&self) -> Arc<coco_app_server::SessionSeqAllocator> {
        Arc::clone(self.state.session_seq_allocator())
    }

    pub fn hub_connector(&self) -> Option<ProcessEventHubEgress> {
        self.hub_connector.clone()
    }

    pub fn open_connection_binding(
        &self,
        app_server: Arc<RemoteAppServer>,
        connection_key: coco_app_server::ConnectionKey,
        external_notifications: Vec<mpsc::Receiver<CoreEvent>>,
        outbound_channel_capacity: usize,
    ) -> RemoteAppServerConnectionBinding {
        open_remote_app_server_connection_binding(
            Arc::clone(&self.state),
            app_server,
            connection_key,
            external_notifications,
            self.turn_drain_timeout,
            outbound_channel_capacity,
        )
    }
}

pub(crate) fn build_remote_app_server_runtime_binding(
    state: &Arc<AppServerHostState>,
    server_config: &coco_config::ServerConfig,
) -> RemoteAppServerRuntimeBinding {
    let event_retention_per_session = server_config_usize(
        server_config.event_retention_per_session,
        REMOTE_APP_SERVER_RETENTION_PER_SESSION,
    );
    install_session_seq_durability(state, event_retention_per_session as i64);

    let app_server = Arc::new(RemoteAppServer::new_with_surface_limits(
        server_config_usize(server_config.max_sessions, REMOTE_APP_SERVER_MAX_SESSIONS),
        event_retention_per_session,
        server_config_surface_limits(server_config),
    ));
    let adapter = RemoteJsonRpcAdapter::with_channel_capacity(
        Arc::clone(&app_server),
        server_config_usize(
            server_config.outbound_queue_frames,
            REMOTE_APP_SERVER_OUTBOUND_QUEUE_FRAMES,
        ),
    );
    let turn_drain_timeout = server_config_duration_secs(
        server_config.turn_drain_timeout_secs,
        super::APP_SERVER_TURN_DRAIN_TIMEOUT,
    );

    RemoteAppServerRuntimeBinding {
        app_server,
        adapter,
        turn_drain_timeout,
    }
}

pub fn open_remote_sidecar_binding(
    state: Arc<AppServerHostState>,
    app_server: Arc<RemoteAppServer>,
    hub_connector: Option<ProcessEventHubEgress>,
    turn_drain_timeout: Duration,
    channel_capacity: usize,
) -> RemoteSidecarHostBinding {
    let (outbound_tx, outbound_rx) = mpsc::channel(channel_capacity);
    let handler = Arc::new(
        AppServerHostHandler::with_local_app_server_and_turn_drain_timeout(
            Arc::clone(&state),
            outbound_tx,
            Arc::clone(&app_server),
            turn_drain_timeout,
        ),
    );
    let outbound_forwarder = spawn_app_server_local_outbound_forwarder(
        app_server,
        state,
        outbound_rx,
        Arc::new(std::sync::RwLock::new(hub_connector)),
    );
    RemoteSidecarHostBinding {
        handler,
        outbound_forwarder,
    }
}

pub async fn shutdown_remote_app_server_host(
    app_server: Arc<RemoteAppServer>,
    state: Arc<AppServerHostState>,
    turn_drain_timeout: Duration,
    shutdown_timeout: Duration,
) -> crate::shutdown::ShutdownDrainOutcome {
    let shutdown_runtimes = app_server
        .list_live_sessions()
        .into_iter()
        .filter_map(|summary| app_server.registry().get(&summary.session_id))
        .map(AppSessionHandle::into_session)
        .collect::<Vec<_>>();
    let app_server_shutdown = crate::shutdown::drain_with_timeout_or_signal(
        shutdown_timeout,
        async move {
            shutdown_local_app_server_sessions(app_server, state, turn_drain_timeout)
                .await
                .map_err(|error| format!("{}: {}", error.code, error.message))
        },
        crate::shutdown::os_interrupt_signal(),
    )
    .await;

    for session_runtime in &shutdown_runtimes {
        crate::shutdown::persist_session_resume_mode(session_runtime).await;
        crate::shutdown::drain_session_memory(session_runtime).await;
    }

    app_server_shutdown
}

fn open_remote_app_server_connection_binding(
    state: Arc<AppServerHostState>,
    app_server: Arc<RemoteAppServer>,
    connection_key: coco_app_server::ConnectionKey,
    external_notifications: Vec<mpsc::Receiver<CoreEvent>>,
    turn_drain_timeout: Duration,
    outbound_channel_capacity: usize,
) -> RemoteAppServerConnectionBinding {
    let (outbound_tx, outbound_rx) = mpsc::channel(outbound_channel_capacity);
    let mut external_forwarders = Vec::new();
    for mut rx in external_notifications {
        let forwarded_tx = outbound_tx.clone();
        external_forwarders.push(tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                let Some(event) = ProcessEvent::from_core_event(event) else {
                    tracing::warn!(
                        "dropping session-scoped event from remote process-event source"
                    );
                    continue;
                };
                if forwarded_tx
                    .send(OutboundMessage::ProcessEvent(event))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }));
    }
    let handler_factory = AppServerHostHandler::with_local_app_server_and_turn_drain_timeout(
        state,
        outbound_tx.clone(),
        app_server,
        turn_drain_timeout,
    );
    let handler =
        coco_app_server::JsonRpcConnectionHandlerFactory::open(&handler_factory, connection_key);
    RemoteAppServerConnectionBinding {
        handler,
        outbound_tx,
        outbound_rx,
        external_forwarders,
    }
}
