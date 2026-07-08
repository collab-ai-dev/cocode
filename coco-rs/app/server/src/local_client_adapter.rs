use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use coco_types::ClientRequest;
use coco_types::SessionEnvelope;
use coco_types::SessionId;
use coco_types::SurfaceId;

use crate::AppLiveSessionSummary;
use crate::AppServer;
use crate::AttachError;
use crate::AttachSurfaceOptions;
use crate::ConnectionKey;
use crate::DetachSurfaceOutcome;
use crate::DisconnectOutcome;
use crate::ServerRequestDelivery;
use crate::SubscribeReplay;
use crate::SurfaceDelivery;
use crate::SurfaceLifecycleDelivery;

const DEFAULT_LOCAL_CHANNEL_CAPACITY: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalClientRequestContext {
    connection: ConnectionKey,
}

impl LocalClientRequestContext {
    pub fn new(connection: ConnectionKey) -> Self {
        Self { connection }
    }

    pub fn connection_key(&self) -> ConnectionKey {
        self.connection
    }
}

pub type LocalClientDispatchError = crate::JsonRpcDispatchError;

pub type LocalClientRequestFuture =
    Pin<Box<dyn Future<Output = Result<serde_json::Value, LocalClientDispatchError>> + Send>>;

pub trait LocalClientRequestHandler: Send + Sync + 'static {
    fn handle_local_client_request(
        &self,
        context: LocalClientRequestContext,
        request: ClientRequest,
    ) -> LocalClientRequestFuture;
}

/// Typed in-process adapter used by local clients.
///
/// This is not a transport shim: it registers a real AppServer connection and
/// returns the same per-connection channels that future transports will drive.
pub struct LocalClientAdapter<H> {
    server: Arc<AppServer<H>>,
    channel_capacity: usize,
}

impl<H> Clone for LocalClientAdapter<H> {
    fn clone(&self) -> Self {
        Self {
            server: Arc::clone(&self.server),
            channel_capacity: self.channel_capacity,
        }
    }
}

impl<H: Clone> LocalClientAdapter<H> {
    pub fn new(server: Arc<AppServer<H>>) -> Self {
        Self::with_channel_capacity(server, DEFAULT_LOCAL_CHANNEL_CAPACITY)
    }

    pub fn with_channel_capacity(server: Arc<AppServer<H>>, channel_capacity: usize) -> Self {
        assert!(
            channel_capacity > 0,
            "local channel capacity must be non-zero"
        );
        Self {
            server,
            channel_capacity,
        }
    }

    pub fn connect(&self) -> LocalClientConnection<H> {
        let connection = ConnectionKey::generate();
        let (event_tx, events) = tokio::sync::mpsc::channel(self.channel_capacity);
        let (request_tx, server_requests) = tokio::sync::mpsc::channel(self.channel_capacity);
        let (lifecycle_tx, lifecycle) = tokio::sync::mpsc::channel(self.channel_capacity);
        self.server.connect_with_request_and_lifecycle_senders(
            connection,
            event_tx,
            request_tx,
            lifecycle_tx,
        );
        LocalClientConnection {
            server: Arc::clone(&self.server),
            connection,
            events,
            server_requests,
            lifecycle,
        }
    }
}

pub struct LocalClientConnection<H> {
    server: Arc<AppServer<H>>,
    connection: ConnectionKey,
    events: tokio::sync::mpsc::Receiver<SurfaceDelivery>,
    server_requests: tokio::sync::mpsc::Receiver<ServerRequestDelivery>,
    lifecycle: tokio::sync::mpsc::Receiver<SurfaceLifecycleDelivery>,
}

impl<H: Clone> LocalClientConnection<H> {
    pub fn connection_key(&self) -> ConnectionKey {
        self.connection
    }

    pub async fn dispatch_client_request<Handler>(
        &self,
        handler: &Handler,
        request: ClientRequest,
    ) -> Result<serde_json::Value, LocalClientDispatchError>
    where
        Handler: LocalClientRequestHandler,
    {
        handler
            .handle_local_client_request(LocalClientRequestContext::new(self.connection), request)
            .await
    }

    pub fn attach_surface(
        &self,
        session_id: SessionId,
        options: AttachSurfaceOptions,
    ) -> Result<LocalClientSurface, AttachError> {
        let surface_id = SurfaceId::generate();
        self.server.attach_surface_with_options(
            self.connection,
            surface_id.clone(),
            session_id.clone(),
            options,
        )?;
        Ok(LocalClientSurface {
            surface_id,
            session_id,
        })
    }

    pub fn subscribe_surface(
        &self,
        session_id: SessionId,
        after_seq: Option<i64>,
        options: AttachSurfaceOptions,
    ) -> Result<LocalClientSubscribeOutcome, AttachError> {
        let surface_id = SurfaceId::generate();
        let replay = self.server.subscribe_surface_with_options(
            self.connection,
            surface_id.clone(),
            session_id.clone(),
            after_seq,
            options,
        )?;
        match replay {
            SubscribeReplay::Replayed(replayed) => Ok(LocalClientSubscribeOutcome::Attached(
                LocalClientSubscription {
                    surface_id,
                    session_id,
                    replayed,
                },
            )),
            SubscribeReplay::SnapshotRequired => Ok(LocalClientSubscribeOutcome::SnapshotRequired),
        }
    }

    pub fn events_mut(&mut self) -> &mut tokio::sync::mpsc::Receiver<SurfaceDelivery> {
        &mut self.events
    }

    pub fn server_requests_mut(
        &mut self,
    ) -> &mut tokio::sync::mpsc::Receiver<ServerRequestDelivery> {
        &mut self.server_requests
    }

    pub fn lifecycle_mut(&mut self) -> &mut tokio::sync::mpsc::Receiver<SurfaceLifecycleDelivery> {
        &mut self.lifecycle
    }

    pub fn detach_surface(&self, surface_id: &SurfaceId) -> DetachSurfaceOutcome {
        self.server
            .detach_surface_for_connection(self.connection, surface_id)
    }

    pub fn list_live_sessions(&self) -> Vec<AppLiveSessionSummary> {
        self.server.list_live_sessions()
    }

    pub fn disconnect(self) -> DisconnectOutcome {
        self.server.disconnect(self.connection)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalClientSurface {
    pub surface_id: SurfaceId,
    pub session_id: SessionId,
}

#[derive(Debug, Clone)]
pub enum LocalClientSubscribeOutcome {
    Attached(LocalClientSubscription),
    SnapshotRequired,
}

#[derive(Debug, Clone)]
pub struct LocalClientSubscription {
    pub surface_id: SurfaceId,
    pub session_id: SessionId,
    pub replayed: Vec<SessionEnvelope>,
}

#[cfg(test)]
#[path = "local_client_adapter.test.rs"]
mod tests;
