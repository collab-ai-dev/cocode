use std::{future::Future, pin::Pin, sync::Arc};

use coco_types::{
    ClientRequest, RequestScope, ServerRequestDelivery, SessionDelivery, SessionEnvelope,
    SessionId, SessionLifecycleEffect, request_scope,
};

use crate::{
    AppLiveSessionSummary, AppServer, AttachError, AttachSessionOptions, ConnectionKey,
    DetachSessionOutcome, DisconnectOutcome, SubscribeReplay,
};

const DEFAULT_LOCAL_CHANNEL_CAPACITY: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalClientRequestContext {
    connection: ConnectionKey,
    scope: RequestScope,
}

impl LocalClientRequestContext {
    pub fn new(connection: ConnectionKey, scope: RequestScope) -> Self {
        Self { connection, scope }
    }

    pub fn connection_key(&self) -> ConnectionKey {
        self.connection
    }

    pub fn request_scope(&self) -> RequestScope {
        self.scope
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
            handle: LocalClientHandle {
                server: Arc::clone(&self.server),
                connection,
            },
            events,
            server_requests,
            lifecycle,
            events_done: false,
            server_requests_done: false,
            lifecycle_done: false,
            disconnected: false,
        }
    }

    pub fn channel_capacity(&self) -> usize {
        self.channel_capacity
    }
}

pub struct LocalClientConnection<H: Clone> {
    handle: LocalClientHandle<H>,
    events: tokio::sync::mpsc::Receiver<SessionDelivery>,
    server_requests: tokio::sync::mpsc::Receiver<ServerRequestDelivery>,
    lifecycle: tokio::sync::mpsc::Receiver<SessionLifecycleEffect>,
    /// Per-channel exhaustion (closed AND drained). `recv` keeps draining a
    /// closed channel's buffered deliveries instead of discarding them.
    events_done: bool,
    server_requests_done: bool,
    lifecycle_done: bool,
    disconnected: bool,
}

pub struct LocalClientHandle<H> {
    server: Arc<AppServer<H>>,
    connection: ConnectionKey,
}

impl<H> Clone for LocalClientHandle<H> {
    fn clone(&self) -> Self {
        Self {
            server: Arc::clone(&self.server),
            connection: self.connection,
        }
    }
}

/// A live delivery received by a local in-process connection.
///
/// Keeping ordinary session events and lifecycle effects in one typed
/// receive path prevents event pumps from silently missing server-driven
/// close/replace transitions.
#[derive(Debug, Clone)]
pub enum LocalClientInbound {
    Event(Box<SessionDelivery>),
    ServerRequest(Box<ServerRequestDelivery>),
    Lifecycle(SessionLifecycleEffect),
}

impl<H: Clone> LocalClientConnection<H> {
    pub fn connection_key(&self) -> ConnectionKey {
        self.handle.connection
    }

    pub fn handle(&self) -> LocalClientHandle<H> {
        self.handle.clone()
    }

    pub fn events_mut(&mut self) -> &mut tokio::sync::mpsc::Receiver<SessionDelivery> {
        &mut self.events
    }

    pub fn server_requests_mut(
        &mut self,
    ) -> &mut tokio::sync::mpsc::Receiver<ServerRequestDelivery> {
        &mut self.server_requests
    }

    pub fn lifecycle_mut(&mut self) -> &mut tokio::sync::mpsc::Receiver<SessionLifecycleEffect> {
        &mut self.lifecycle
    }

    pub async fn recv(&mut self) -> Option<LocalClientInbound> {
        loop {
            if self.events_done && self.server_requests_done && self.lifecycle_done {
                return None;
            }
            // `mpsc::Receiver::recv` keeps yielding buffered deliveries after
            // the senders drop and returns `None` only once drained, so a
            // server-side disconnect never discards already-routed messages.
            tokio::select! {
                event = self.events.recv(), if !self.events_done => match event {
                    Some(event) => return Some(LocalClientInbound::Event(Box::new(event))),
                    None => self.events_done = true,
                },
                request = self.server_requests.recv(), if !self.server_requests_done => match request {
                    Some(request) => {
                        return Some(LocalClientInbound::ServerRequest(Box::new(request)));
                    }
                    None => self.server_requests_done = true,
                },
                lifecycle = self.lifecycle.recv(), if !self.lifecycle_done => match lifecycle {
                    Some(lifecycle) => return Some(LocalClientInbound::Lifecycle(lifecycle)),
                    None => self.lifecycle_done = true,
                },
            }
        }
    }

    pub fn disconnect(mut self) -> DisconnectOutcome {
        let outcome = self.handle.server.disconnect(self.handle.connection);
        self.disconnected = true;
        outcome
    }
}

impl<H: Clone> LocalClientHandle<H> {
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
        let scope = request_scope(request.method());
        handler
            .handle_local_client_request(
                LocalClientRequestContext::new(self.connection, scope),
                request,
            )
            .await
    }

    pub fn attach_session(
        &self,
        session_id: SessionId,
        options: AttachSessionOptions,
    ) -> Result<LocalClientSession, AttachError> {
        self.server
            .attach_live_session(self.connection, session_id.clone(), options)?;
        Ok(LocalClientSession { session_id })
    }

    pub fn subscribe_session(
        &self,
        session_id: SessionId,
        after_seq: Option<i64>,
        options: AttachSessionOptions,
    ) -> Result<LocalClientSubscribeOutcome, AttachError> {
        let replay = self.server.subscribe_live_session(
            self.connection,
            session_id.clone(),
            after_seq,
            options,
        )?;
        match replay {
            SubscribeReplay::Replayed(replayed) => Ok(LocalClientSubscribeOutcome::Attached(
                LocalClientSubscription {
                    session_id,
                    replayed,
                },
            )),
            SubscribeReplay::SnapshotRequired => Ok(LocalClientSubscribeOutcome::SnapshotRequired),
        }
    }

    pub fn detach_session(&self, session_id: &SessionId) -> DetachSessionOutcome {
        self.server
            .detach_session_for_connection(self.connection, session_id)
    }

    pub fn list_live_sessions(&self) -> Vec<AppLiveSessionSummary> {
        self.server.list_live_sessions()
    }

    pub fn disconnect(&self) -> DisconnectOutcome {
        self.server.disconnect(self.connection)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalClientSession {
    pub session_id: SessionId,
}

#[derive(Debug, Clone)]
pub enum LocalClientSubscribeOutcome {
    Attached(LocalClientSubscription),
    SnapshotRequired,
}

#[derive(Debug, Clone)]
pub struct LocalClientSubscription {
    pub session_id: SessionId,
    pub replayed: Vec<SessionEnvelope>,
}

impl<H: Clone> Drop for LocalClientConnection<H> {
    fn drop(&mut self) {
        if !self.disconnected {
            self.handle.server.disconnect(self.handle.connection);
            self.disconnected = true;
        }
    }
}

#[cfg(test)]
#[path = "local_client_adapter.test.rs"]
mod tests;
