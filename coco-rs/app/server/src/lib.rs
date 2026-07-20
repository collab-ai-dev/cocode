//! App-server lifecycle, routing, and protocol adapters.
//!
//! The crate owns opaque live-session slots, connection/session attachments,
//! durable replay, local/JSON-RPC adapters, and listener supervision. Concrete
//! session-runtime construction and close behavior are supplied by the
//! application host.

mod activity;
mod app_server;
mod json_rpc_adapter;
mod local_client_adapter;
mod registration_policy;
mod registry;
mod session_data;
mod session_seq;

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::atomic::{AtomicI64, Ordering},
};

use chrono::{DateTime, Utc};
use coco_error::{ErrorExt, Location, StatusCode, stack_trace_debug};
use coco_types::{
    RequestId, ServerCancelRequestParams, ServerRequest, ServerRequestDelivery, SessionAccess,
    SessionDelivery, SessionEnvelope, SessionId, SessionLifecycleEffect, TurnId,
};
use snafu::Snafu;

pub use activity::SessionActivityTracker;
pub use app_server::{
    AppCloseCommit, AppCloseStart, AppLiveSessionSummary, AppLoadStart, AppReplaceCommit,
    AppReplaceStart, AppServer, AppServerError, AppShutdownSession, AppShutdownStart,
    ResolvedServerRequest, ServerRequestErrorReply, ServerRequestReply, ServerRequestResolution,
    ValidatedSession,
};
pub use json_rpc_adapter::{
    JsonRpcAdapter, JsonRpcAdapterConnection, JsonRpcAdapterError, JsonRpcConnectionHandlerFactory,
    JsonRpcConnectionOwnerError, JsonRpcDispatchError, JsonRpcRequestContext, JsonRpcRequestFuture,
    JsonRpcRequestHandler, JsonRpcServerRequestResponse, PendingJsonRpcServerRequest,
};
pub use local_client_adapter::{
    LocalClientAdapter, LocalClientConnection, LocalClientDispatchError, LocalClientHandle,
    LocalClientInbound, LocalClientRequestContext, LocalClientRequestFuture,
    LocalClientRequestHandler, LocalClientSession, LocalClientSubscribeOutcome,
    LocalClientSubscription,
};
pub use registration_policy::{
    SessionEgress, SessionRegistrationPolicy, SessionTopology, SessionVisibility,
};
pub use registry::{
    CloseCompletion, CloseStart, CompleteLoadFailure, LiveSessionRegistry, LoadCompletion,
    LoadStart, RegistryError, ReplaceCommit, ReplaceStart,
};
pub use session_data::{
    AppSessionDataError, AppSessionDataHandle, AppSessionDataRequest, AppSessionDataSource,
    LiveSessionDataMessage, LiveSessionDataSnapshot, SessionDataProjectionError, SessionPage,
    TranscriptTurnEntry, derive_session_turn_summaries, page_session_items,
    parse_session_data_cursor, parse_session_data_limit, session_data_page,
};
pub use session_seq::{SessionSeqAllocator, SessionSeqPersistHook, WATERMARK_PERSIST_INTERVAL};

static NEXT_CONNECTION_KEY: AtomicI64 = AtomicI64::new(1);
static NEXT_SERVER_REQUEST_ID: AtomicI64 = AtomicI64::new(1);

/// Private server-side routing key for one transport connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConnectionKey(i64);

impl ConnectionKey {
    pub fn generate() -> Self {
        Self(NEXT_CONNECTION_KEY.fetch_add(1, Ordering::Relaxed))
    }
}

pub type OutboundSender = tokio::sync::mpsc::Sender<SessionDelivery>;
pub type ServerRequestSender = tokio::sync::mpsc::Sender<ServerRequestDelivery>;
pub type SessionLifecycleSender = tokio::sync::mpsc::Sender<SessionLifecycleEffect>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotificationPrefs {
    pub protocol: bool,
    pub stream: bool,
    pub tui: bool,
}

impl Default for NotificationPrefs {
    fn default() -> Self {
        Self {
            protocol: true,
            stream: true,
            tui: true,
        }
    }
}

impl NotificationPrefs {
    fn accepts(self, envelope: &SessionEnvelope) -> bool {
        match &envelope.event {
            coco_types::CoreEvent::Protocol(_) => self.protocol,
            coco_types::CoreEvent::Stream(_) => self.stream,
            coco_types::CoreEvent::Tui(_) => self.tui,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionAttachment {
    pub connection: ConnectionKey,
    pub session_id: SessionId,
    pub notification_prefs: NotificationPrefs,
    pub last_delivered_seq: i64,
    pub attached_at: DateTime<Utc>,
}

/// Authorization held by one connection for one session.
///
/// Grants are independent of live event attachments. Closing a session drops
/// its attachment but preserves the grant for durable reads and explicit
/// deletion until the connection disconnects or revokes it.
#[derive(Debug, Clone)]
pub struct SessionGrant {
    pub connection: ConnectionKey,
    pub session_id: SessionId,
    pub access: SessionAccess,
    pub granted_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct AttachSessionOptions {
    pub grant: SessionAccess,
    pub notification_prefs: NotificationPrefs,
    pub last_delivered_seq: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionLimits {
    pub max_attached_sessions_per_connection: usize,
    pub max_connections_per_session: usize,
}

impl Default for ConnectionLimits {
    fn default() -> Self {
        Self {
            max_attached_sessions_per_connection: 8,
            max_connections_per_session: 16,
        }
    }
}

impl AttachSessionOptions {
    pub fn full() -> Self {
        Self {
            grant: SessionAccess::Full,
            notification_prefs: NotificationPrefs::default(),
            last_delivered_seq: 0,
        }
    }

    pub fn read_only() -> Self {
        Self {
            grant: SessionAccess::ReadOnly,
            notification_prefs: NotificationPrefs::default(),
            last_delivered_seq: 0,
        }
    }
}

#[stack_trace_debug]
#[derive(Clone, Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum AttachError {
    #[snafu(display("connection is not registered"))]
    ConnectionNotRegistered {
        connection: ConnectionKey,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display(
        "connection attachment limit reached ({max_attached_sessions_per_connection})"
    ))]
    ConnectionAttachmentLimit {
        connection: ConnectionKey,
        max_attached_sessions_per_connection: usize,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display(
        "session {session_id} connection limit reached ({max_connections_per_session})"
    ))]
    SessionConnectionLimit {
        session_id: SessionId,
        max_connections_per_session: usize,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("session {session_id} is closing"))]
    SessionClosing {
        session_id: SessionId,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("session {session_id} is not a live session"))]
    SessionNotFound {
        session_id: SessionId,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("session {session_id} attach replay queue is unavailable"))]
    ReplayQueueUnavailable {
        session_id: SessionId,
        #[snafu(implicit)]
        location: Location,
    },
}

impl ErrorExt for AttachError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::ConnectionNotRegistered { .. } => StatusCode::InvalidArguments,
            Self::ConnectionAttachmentLimit { .. } | Self::SessionConnectionLimit { .. } => {
                StatusCode::ResourcesExhausted
            }
            Self::SessionClosing { .. } | Self::ReplayQueueUnavailable { .. } => {
                StatusCode::Cancelled
            }
            Self::SessionNotFound { .. } => StatusCode::FileNotFound,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionAccessError {
    MissingGrant {
        connection: ConnectionKey,
        session_id: SessionId,
    },
    NotAttached {
        connection: ConnectionKey,
        session_id: SessionId,
    },
    ReadOnly {
        connection: ConnectionKey,
        session_id: SessionId,
    },
}

impl std::fmt::Display for SessionAccessError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingGrant { session_id, .. } => {
                write!(
                    formatter,
                    "connection has no grant for session {session_id}"
                )
            }
            Self::NotAttached { session_id, .. } => {
                write!(
                    formatter,
                    "connection is not attached to session {session_id}"
                )
            }
            Self::ReadOnly { session_id, .. } => {
                write!(formatter, "session {session_id} grant is read-only")
            }
        }
    }
}

impl std::error::Error for SessionAccessError {}

#[derive(Debug, Clone)]
pub enum SubscribeReplay {
    Replayed(Vec<SessionEnvelope>),
    SnapshotRequired,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteOutcome {
    pub delivered: usize,
    pub disconnected: Vec<ConnectionKey>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DisconnectOutcome {
    pub detached_sessions: Vec<SessionId>,
    pub cancelled_requests: Vec<RequestId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DetachSessionOutcome {
    pub detached: bool,
    pub cancelled_requests: Vec<RequestId>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionConnectionCounts {
    pub full: usize,
    pub read_only: usize,
}

impl SessionConnectionCounts {
    pub fn total(self) -> usize {
        self.full + self.read_only
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceAttachmentOutcome {
    pub old_session_id: SessionId,
    pub new_session_id: SessionId,
    pub calling_connection: ConnectionKey,
    pub detached_connections: Vec<ConnectionKey>,
    pub cancelled_requests: Vec<RequestId>,
}

#[derive(Debug)]
pub enum ReplaceAttachmentError {
    Access(SessionAccessError),
    Attach(AttachError),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CloseSessionAttachmentsOutcome {
    pub detached_connections: Vec<ConnectionKey>,
    pub cancelled_requests: Vec<RequestId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetedSessionLifecycleEffect {
    pub connection: ConnectionKey,
    pub effect: SessionLifecycleEffect,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LifecycleRouteOutcome {
    pub delivered: usize,
    pub disconnected: Vec<ConnectionKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingServerRequest {
    pub request_id: RequestId,
    pub session_id: SessionId,
    pub turn_id: Option<TurnId>,
    pub minted: i64,
}

#[derive(Debug, Clone)]
pub struct PendingServerRequestReplay {
    pub pending: PendingServerRequest,
    pub request: ServerRequest,
}

/// Selects who may answer a server-initiated request.
///
/// Human interaction is offered to every full-access connection and the first
/// valid reply wins. Connection-hosted callbacks stay bound to the transport
/// that registered them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerRequestAudience {
    AllFullConnections,
    Connection(ConnectionKey),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ConnectionCallback {
    Hook(String),
    McpServer(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerRequestReplyKind {
    Approval,
    UserInput,
    Elicitation,
    McpRouteMessage,
    HookCallback,
}

#[derive(Debug, Clone)]
struct PendingServerRequestEntry {
    pending: PendingServerRequest,
    request: ServerRequest,
    audience: ServerRequestAudience,
    recipients: HashSet<ConnectionKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompleteServerRequestError {
    NotFound {
        request_id: RequestId,
    },
    WrongSession {
        request_id: RequestId,
        expected_session_id: SessionId,
        actual_session_id: SessionId,
    },
    Access(SessionAccessError),
    NotRecipient {
        request_id: RequestId,
        connection: ConnectionKey,
    },
    WrongReplyKind {
        request_id: RequestId,
        expected: ServerRequestReplyKind,
        actual: ServerRequestReplyKind,
    },
}

/// Result of one recipient cancelling its participation in a server request.
///
/// A broadcast remains pending while another Full connection can answer it.
/// Connection-targeted requests, and broadcasts whose last recipient leaves,
/// are cancelled completely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelServerRequestOutcome {
    Withdrawn,
    Cancelled(PendingServerRequest),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerRequestRouteOutcome {
    pub pending: PendingServerRequest,
    pub delivered: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerRequestRouteError {
    NoFullConnection {
        session_id: SessionId,
    },
    QueueUnavailable {
        request_id: RequestId,
        session_id: SessionId,
    },
    /// `ServerRequest::CancelRequest` is a notification, never a pending
    /// request; routing one is a caller bug rejected at prepare time.
    CancellationNotRoutable {
        session_id: SessionId,
    },
}

/// How an error reply from one recipient resolved a pending request.
///
/// A broadcast stays answerable by peers when one recipient fails to handle
/// it — "first valid reply wins" must not let an error reply consume the
/// request for everyone. A connection-targeted request has exactly one
/// possible responder, so its error is the final answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorReplyDisposition {
    /// Connection-targeted request: the error completes it and must reach
    /// the reply waiter.
    CompletedTargeted(PendingServerRequest),
    /// Broadcast with other recipients left: only the sender is withdrawn.
    Withdrawn,
    /// Broadcast whose last recipient errored: the request is cancelled.
    CancelledLast(PendingServerRequest),
}

/// Single-lock routing state for connections, session attachments, and replay.
#[derive(Debug)]
pub struct RoutingState {
    retention_per_session: usize,
    connection_limits: ConnectionLimits,
    grants: HashMap<(ConnectionKey, SessionId), SessionGrant>,
    attachments: HashMap<(ConnectionKey, SessionId), SessionAttachment>,
    session_to_connections: HashMap<SessionId, HashSet<ConnectionKey>>,
    connection_to_sessions: HashMap<ConnectionKey, HashSet<SessionId>>,
    connection_senders: HashMap<ConnectionKey, OutboundSender>,
    request_senders: HashMap<ConnectionKey, ServerRequestSender>,
    lifecycle_senders: HashMap<ConnectionKey, SessionLifecycleSender>,
    /// Registration stack per callback, most recent registrant first. Routing
    /// targets the front entry; pruning a disconnecting/detaching connection
    /// falls back to the next still-attached registrant instead of orphaning
    /// the callback for the rest of the session.
    callback_owners: HashMap<(SessionId, ConnectionCallback), Vec<ConnectionKey>>,
    rings: HashMap<SessionId, RetentionRing>,
    pending_server_requests: HashMap<RequestId, PendingServerRequestEntry>,
    pending_requests_by_session: HashMap<SessionId, HashSet<RequestId>>,
    pending_requests_by_turn: HashMap<TurnId, HashSet<RequestId>>,
    /// Pending entries removed by an internal `disconnect` (slow consumer)
    /// whose reply waiters still need cancellation. `AppServer` wrappers drain
    /// this after releasing the routing lock; leaving an id here past the next
    /// wrapper call would strand its waiter until the request timeout.
    orphaned_waiter_requests: Vec<RequestId>,
}

impl RoutingState {
    pub fn new(retention_per_session: usize) -> Self {
        Self::new_with_connection_limits(retention_per_session, ConnectionLimits::default())
    }

    pub fn new_with_connection_limits(
        retention_per_session: usize,
        connection_limits: ConnectionLimits,
    ) -> Self {
        assert!(
            connection_limits.max_attached_sessions_per_connection > 0,
            "max attached sessions per connection must be non-zero"
        );
        assert!(
            connection_limits.max_connections_per_session > 0,
            "max connections per session must be non-zero"
        );
        Self {
            retention_per_session,
            connection_limits,
            grants: HashMap::new(),
            attachments: HashMap::new(),
            session_to_connections: HashMap::new(),
            connection_to_sessions: HashMap::new(),
            connection_senders: HashMap::new(),
            request_senders: HashMap::new(),
            lifecycle_senders: HashMap::new(),
            callback_owners: HashMap::new(),
            rings: HashMap::new(),
            pending_server_requests: HashMap::new(),
            pending_requests_by_session: HashMap::new(),
            pending_requests_by_turn: HashMap::new(),
            orphaned_waiter_requests: Vec::new(),
        }
    }

    /// Drain request ids whose pending entries were removed by an internal
    /// disconnect. The caller must cancel the corresponding reply waiters
    /// after releasing the routing lock.
    pub(crate) fn take_orphaned_waiter_cancellations(&mut self) -> Vec<RequestId> {
        std::mem::take(&mut self.orphaned_waiter_requests)
    }

    pub fn connect(&mut self, connection: ConnectionKey, sender: OutboundSender) {
        self.connection_senders.insert(connection, sender);
        self.request_senders.remove(&connection);
        self.lifecycle_senders.remove(&connection);
        self.connection_to_sessions.entry(connection).or_default();
    }

    pub fn connect_with_request_sender(
        &mut self,
        connection: ConnectionKey,
        sender: OutboundSender,
        request_sender: ServerRequestSender,
    ) {
        self.connect(connection, sender);
        self.request_senders.insert(connection, request_sender);
    }

    pub fn connect_with_lifecycle_sender(
        &mut self,
        connection: ConnectionKey,
        sender: OutboundSender,
        lifecycle_sender: SessionLifecycleSender,
    ) {
        self.connect(connection, sender);
        self.lifecycle_senders.insert(connection, lifecycle_sender);
    }

    pub fn connect_with_request_and_lifecycle_senders(
        &mut self,
        connection: ConnectionKey,
        sender: OutboundSender,
        request_sender: ServerRequestSender,
        lifecycle_sender: SessionLifecycleSender,
    ) {
        self.connect_with_request_sender(connection, sender, request_sender);
        self.lifecycle_senders.insert(connection, lifecycle_sender);
    }

    pub fn attach_session(
        &mut self,
        connection: ConnectionKey,
        session_id: SessionId,
        options: AttachSessionOptions,
    ) -> Result<(), AttachError> {
        self.ensure_attach_capacity(connection, &session_id, None)?;
        self.attach_session_after_validation(connection, session_id, options)
    }

    fn attach_session_after_validation(
        &mut self,
        connection: ConnectionKey,
        session_id: SessionId,
        mut options: AttachSessionOptions,
    ) -> Result<(), AttachError> {
        let key = (connection, session_id.clone());
        let was_full = self
            .grants
            .get(&key)
            .is_some_and(|grant| grant.access == SessionAccess::Full);
        if was_full {
            options.grant = SessionAccess::Full;
        }
        self.grants
            .entry(key.clone())
            .and_modify(|grant| grant.access = options.grant)
            .or_insert_with(|| SessionGrant {
                connection,
                session_id: session_id.clone(),
                access: options.grant,
                granted_at: Utc::now(),
            });
        self.attachments.insert(
            key,
            SessionAttachment {
                connection,
                session_id: session_id.clone(),
                notification_prefs: options.notification_prefs,
                last_delivered_seq: options.last_delivered_seq,
                attached_at: Utc::now(),
            },
        );
        self.session_to_connections
            .entry(session_id.clone())
            .or_default()
            .insert(connection);
        self.connection_to_sessions
            .entry(connection)
            .or_default()
            .insert(session_id.clone());

        if options.grant != SessionAccess::Full || was_full {
            return Ok(());
        }
        let Some(sender) = self.request_senders.get(&connection).cloned() else {
            return Ok(());
        };
        let replay_ids = self.pending_broadcast_request_ids_for_session(&session_id);
        for request_id in replay_ids {
            let Some(entry) = self.pending_server_requests.get_mut(&request_id) else {
                continue;
            };
            entry.recipients.insert(connection);
            let delivery = ServerRequestDelivery {
                session_id: session_id.clone(),
                request_id: request_id.clone(),
                request: entry.request.clone(),
            };
            if sender.try_send(delivery).is_err() {
                self.disconnect(connection);
                return ReplayQueueUnavailableSnafu { session_id }.fail();
            }
        }
        Ok(())
    }

    fn ensure_attach_capacity(
        &self,
        connection: ConnectionKey,
        session_id: &SessionId,
        replacing_session_id: Option<&SessionId>,
    ) -> Result<(), AttachError> {
        if !self.connection_senders.contains_key(&connection) {
            return ConnectionNotRegisteredSnafu { connection }.fail();
        }
        if self
            .attachments
            .contains_key(&(connection, session_id.clone()))
        {
            return Ok(());
        }
        let attached_sessions = self
            .connection_to_sessions
            .get(&connection)
            .map_or(0, HashSet::len);
        let replacing_attached = replacing_session_id.is_some_and(|replacing| {
            self.attachments
                .contains_key(&(connection, replacing.clone()))
        });
        if attached_sessions >= self.connection_limits.max_attached_sessions_per_connection
            && !replacing_attached
        {
            return ConnectionAttachmentLimitSnafu {
                connection,
                max_attached_sessions_per_connection: self
                    .connection_limits
                    .max_attached_sessions_per_connection,
            }
            .fail();
        }
        let connections = self
            .session_to_connections
            .get(session_id)
            .map_or(0, HashSet::len);
        if connections >= self.connection_limits.max_connections_per_session {
            return SessionConnectionLimitSnafu {
                session_id: session_id.clone(),
                max_connections_per_session: self.connection_limits.max_connections_per_session,
            }
            .fail();
        }
        Ok(())
    }

    pub fn subscribe(
        &mut self,
        connection: ConnectionKey,
        session_id: SessionId,
        after_seq: Option<i64>,
        options: AttachSessionOptions,
    ) -> Result<SubscribeReplay, AttachError> {
        let Some(after_seq) = after_seq else {
            return Ok(SubscribeReplay::SnapshotRequired);
        };
        let replay = self
            .rings
            .get(&session_id)
            .map_or(RingReplay::Available(Vec::new()), |ring| {
                ring.replay_after(after_seq)
            });
        match replay {
            RingReplay::Available(envelopes) => {
                self.attach_session(connection, session_id, options)?;
                Ok(SubscribeReplay::Replayed(envelopes))
            }
            RingReplay::TooOld => Ok(SubscribeReplay::SnapshotRequired),
        }
    }

    pub fn route_envelope(&mut self, envelope: SessionEnvelope) -> RouteOutcome {
        if envelope.is_durable() {
            self.ring_for(envelope.session_id.clone())
                .append(envelope.clone());
        }
        let Some(connections) = self.session_to_connections.get(&envelope.session_id) else {
            return RouteOutcome::default();
        };
        let deliveries: Vec<(ConnectionKey, OutboundSender)> = connections
            .iter()
            .filter_map(|connection| {
                let key = (*connection, envelope.session_id.clone());
                let attachment = self.attachments.get(&key)?;
                if !attachment.notification_prefs.accepts(&envelope) {
                    return None;
                }
                Some((
                    *connection,
                    self.connection_senders.get(connection)?.clone(),
                ))
            })
            .collect();
        let mut outcome = RouteOutcome::default();
        for (connection, sender) in deliveries {
            match sender.try_send(SessionDelivery {
                envelope: envelope.clone(),
            }) {
                Ok(()) => {
                    outcome.delivered += 1;
                    if let Some(seq) = envelope.session_seq
                        && let Some(attachment) = self
                            .attachments
                            .get_mut(&(connection, envelope.session_id.clone()))
                    {
                        attachment.last_delivered_seq = seq;
                    }
                }
                Err(tokio::sync::mpsc::error::TrySendError::Full(_))
                | Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    outcome.disconnected.push(connection);
                    self.disconnect(connection);
                }
            }
        }
        outcome
    }

    pub fn route_lifecycle_effects(
        &mut self,
        effects: Vec<TargetedSessionLifecycleEffect>,
    ) -> LifecycleRouteOutcome {
        let mut outcome = LifecycleRouteOutcome::default();
        for targeted in effects {
            if outcome.disconnected.contains(&targeted.connection) {
                continue;
            }
            let Some(sender) = self.lifecycle_senders.get(&targeted.connection).cloned() else {
                continue;
            };
            match sender.try_send(targeted.effect) {
                Ok(()) => outcome.delivered += 1,
                Err(tokio::sync::mpsc::error::TrySendError::Full(_))
                | Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    outcome.disconnected.push(targeted.connection);
                    self.disconnect(targeted.connection);
                }
            }
        }
        outcome
    }

    /// Fire-and-forget composition of prepare + publish. Test seam only:
    /// production requests go through `AppServer` so a reply waiter and a
    /// timeout are always installed before publication.
    #[cfg(test)]
    pub(crate) fn route_server_request(
        &mut self,
        session_id: SessionId,
        turn_id: Option<TurnId>,
        request: ServerRequest,
    ) -> Result<ServerRequestRouteOutcome, ServerRequestRouteError> {
        let pending = self.prepare_server_request(
            ServerRequestAudience::AllFullConnections,
            session_id,
            turn_id,
            request,
        )?;
        self.publish_prepared_server_request(&pending.request_id)
    }

    /// Targeted-audience sibling of [`Self::route_server_request`]; same
    /// test-seam-only caveat.
    #[cfg(test)]
    pub(crate) fn route_server_request_to(
        &mut self,
        audience: ServerRequestAudience,
        session_id: SessionId,
        turn_id: Option<TurnId>,
        request: ServerRequest,
    ) -> Result<ServerRequestRouteOutcome, ServerRequestRouteError> {
        let pending = self.prepare_server_request(audience, session_id, turn_id, request)?;
        self.publish_prepared_server_request(&pending.request_id)
    }

    pub(crate) fn prepare_server_request(
        &mut self,
        audience: ServerRequestAudience,
        session_id: SessionId,
        turn_id: Option<TurnId>,
        request: ServerRequest,
    ) -> Result<PendingServerRequest, ServerRequestRouteError> {
        if matches!(request, ServerRequest::CancelRequest(_)) {
            return Err(ServerRequestRouteError::CancellationNotRoutable { session_id });
        }
        let targets: Vec<ConnectionKey> = self
            .session_to_connections
            .get(&session_id)
            .into_iter()
            .flatten()
            .filter_map(|connection| {
                self.attachments.get(&(*connection, session_id.clone()))?;
                if self.session_access(*connection, &session_id) != Some(SessionAccess::Full) {
                    return None;
                }
                self.request_senders.get(connection)?;
                match audience {
                    ServerRequestAudience::AllFullConnections => Some(*connection),
                    ServerRequestAudience::Connection(target) if target == *connection => {
                        Some(*connection)
                    }
                    ServerRequestAudience::Connection(_) => None,
                }
            })
            .collect();
        if targets.is_empty() {
            return Err(ServerRequestRouteError::NoFullConnection { session_id });
        }
        let pending = Self::new_pending_server_request(session_id, turn_id);
        self.insert_pending_server_request(PendingServerRequestEntry {
            pending: pending.clone(),
            request,
            audience,
            recipients: targets.into_iter().collect(),
        });
        Ok(pending)
    }

    pub(crate) fn publish_prepared_server_request(
        &mut self,
        request_id: &RequestId,
    ) -> Result<ServerRequestRouteOutcome, ServerRequestRouteError> {
        let Some(entry) = self.pending_server_requests.get(request_id) else {
            unreachable!("prepared server request must exist before publication")
        };
        let pending = entry.pending.clone();
        let request = entry.request.clone();
        let targets: Vec<_> = entry
            .recipients
            .iter()
            .filter_map(|connection| {
                Some((*connection, self.request_senders.get(connection)?.clone()))
            })
            .collect();
        let mut delivered = 0;
        for (connection, sender) in targets {
            let delivery = ServerRequestDelivery {
                session_id: pending.session_id.clone(),
                request_id: pending.request_id.clone(),
                request: request.clone(),
            };
            match sender.try_send(delivery) {
                Ok(()) => delivered += 1,
                Err(tokio::sync::mpsc::error::TrySendError::Full(_))
                | Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    self.disconnect(connection);
                }
            }
        }
        if delivered == 0 {
            self.remove_pending_server_request(&pending.request_id);
            return Err(ServerRequestRouteError::QueueUnavailable {
                request_id: pending.request_id,
                session_id: pending.session_id,
            });
        }
        Ok(ServerRequestRouteOutcome { pending, delivered })
    }

    pub fn complete_server_request(
        &mut self,
        connection: ConnectionKey,
        session_id: &SessionId,
        request_id: &RequestId,
        reply_kind: Option<ServerRequestReplyKind>,
    ) -> Result<PendingServerRequest, CompleteServerRequestError> {
        let Some(entry) = self.pending_server_requests.get(request_id) else {
            return Err(CompleteServerRequestError::NotFound {
                request_id: request_id.clone(),
            });
        };
        let pending = &entry.pending;
        if pending.session_id != *session_id {
            return Err(CompleteServerRequestError::WrongSession {
                request_id: request_id.clone(),
                expected_session_id: pending.session_id.clone(),
                actual_session_id: session_id.clone(),
            });
        }
        self.require_full(connection, session_id)
            .map_err(CompleteServerRequestError::Access)?;
        if !entry.recipients.contains(&connection) {
            return Err(CompleteServerRequestError::NotRecipient {
                request_id: request_id.clone(),
                connection,
            });
        }
        if let (Some(expected), Some(actual)) =
            (server_request_reply_kind(&entry.request), reply_kind)
            && expected != actual
        {
            return Err(CompleteServerRequestError::WrongReplyKind {
                request_id: request_id.clone(),
                expected,
                actual,
            });
        }
        let losing_connections: Vec<_> = entry
            .recipients
            .iter()
            .copied()
            .filter(|recipient| *recipient != connection)
            .collect();
        let pending = self
            .remove_pending_server_request(request_id)
            .ok_or_else(|| CompleteServerRequestError::NotFound {
                request_id: request_id.clone(),
            })?;
        self.notify_server_request_losers(
            &pending.session_id,
            request_id,
            losing_connections,
            "answered by another full-access connection",
        );
        Ok(pending)
    }

    /// Resolve an error reply from `connection` per audience: complete a
    /// connection-targeted request (the sole responder failed), but only
    /// withdraw the sender from a broadcast so peers can still answer.
    pub fn resolve_error_reply(
        &mut self,
        connection: ConnectionKey,
        session_id: &SessionId,
        request_id: &RequestId,
    ) -> Result<ErrorReplyDisposition, CompleteServerRequestError> {
        let Some(entry) = self.pending_server_requests.get(request_id) else {
            return Err(CompleteServerRequestError::NotFound {
                request_id: request_id.clone(),
            });
        };
        if entry.pending.session_id != *session_id {
            return Err(CompleteServerRequestError::WrongSession {
                request_id: request_id.clone(),
                expected_session_id: entry.pending.session_id.clone(),
                actual_session_id: session_id.clone(),
            });
        }
        self.require_full(connection, session_id)
            .map_err(CompleteServerRequestError::Access)?;
        if !entry.recipients.contains(&connection) {
            return Err(CompleteServerRequestError::NotRecipient {
                request_id: request_id.clone(),
                connection,
            });
        }
        match entry.audience {
            ServerRequestAudience::Connection(_) => self
                .remove_pending_server_request(request_id)
                .map(ErrorReplyDisposition::CompletedTargeted)
                .ok_or_else(|| CompleteServerRequestError::NotFound {
                    request_id: request_id.clone(),
                }),
            ServerRequestAudience::AllFullConnections => {
                if entry.recipients.len() > 1 {
                    let Some(entry) = self.pending_server_requests.get_mut(request_id) else {
                        return Err(CompleteServerRequestError::NotFound {
                            request_id: request_id.clone(),
                        });
                    };
                    entry.recipients.remove(&connection);
                    return Ok(ErrorReplyDisposition::Withdrawn);
                }
                self.cancel_pending_server_request(
                    request_id,
                    "last full-access recipient could not answer",
                )
                .map(ErrorReplyDisposition::CancelledLast)
                .ok_or_else(|| CompleteServerRequestError::NotFound {
                    request_id: request_id.clone(),
                })
            }
        }
    }

    pub fn cancel_server_request_for_connection(
        &mut self,
        request_id: &RequestId,
        connection: ConnectionKey,
    ) -> Result<CancelServerRequestOutcome, CompleteServerRequestError> {
        let Some(entry) = self.pending_server_requests.get(request_id) else {
            return Err(CompleteServerRequestError::NotFound {
                request_id: request_id.clone(),
            });
        };
        let session_id = entry.pending.session_id.clone();
        self.require_full(connection, &session_id)
            .map_err(CompleteServerRequestError::Access)?;
        if !entry.recipients.contains(&connection) {
            return Err(CompleteServerRequestError::NotRecipient {
                request_id: request_id.clone(),
                connection,
            });
        }
        if entry.recipients.len() > 1 {
            let Some(entry) = self.pending_server_requests.get_mut(request_id) else {
                return Err(CompleteServerRequestError::NotFound {
                    request_id: request_id.clone(),
                });
            };
            entry.recipients.remove(&connection);
            return Ok(CancelServerRequestOutcome::Withdrawn);
        }
        self.cancel_pending_server_request(request_id, "last full-access recipient cancelled")
            .map(CancelServerRequestOutcome::Cancelled)
            .ok_or_else(|| CompleteServerRequestError::NotFound {
                request_id: request_id.clone(),
            })
    }

    pub fn cancel_turn_server_requests(&mut self, turn_id: &TurnId) -> Vec<RequestId> {
        let Some(requests) = self.pending_requests_by_turn.remove(turn_id) else {
            return Vec::new();
        };
        requests
            .into_iter()
            .filter(|request_id| {
                self.cancel_pending_server_request(request_id, "owning turn ended")
                    .is_some()
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn pending_server_request_replays_for_session(
        &self,
        session_id: &SessionId,
    ) -> Vec<PendingServerRequestReplay> {
        let Some(requests) = self.pending_requests_by_session.get(session_id) else {
            return Vec::new();
        };
        let mut replays: Vec<_> = requests
            .iter()
            .filter_map(|request_id| {
                Some(PendingServerRequestReplay {
                    pending: self
                        .pending_server_requests
                        .get(request_id)?
                        .pending
                        .clone(),
                    request: self
                        .pending_server_requests
                        .get(request_id)?
                        .request
                        .clone(),
                })
            })
            .collect();
        replays.sort_by_key(|replay| replay.pending.minted);
        replays
    }

    pub fn replace_calling_attachment(
        &mut self,
        connection: ConnectionKey,
        old_session_id: &SessionId,
        new_session_id: SessionId,
    ) -> Result<ReplaceAttachmentOutcome, ReplaceAttachmentError> {
        self.require_full(connection, old_session_id)
            .map_err(ReplaceAttachmentError::Access)?;
        let Some(attachment) = self.attachments.get(&(connection, old_session_id.clone())) else {
            return Err(ReplaceAttachmentError::Access(
                SessionAccessError::NotAttached {
                    connection,
                    session_id: old_session_id.clone(),
                },
            ));
        };
        let calling_options = AttachSessionOptions {
            grant: SessionAccess::Full,
            notification_prefs: attachment.notification_prefs,
            last_delivered_seq: attachment.last_delivered_seq,
        };
        self.ensure_attach_capacity(connection, &new_session_id, Some(old_session_id))
            .map_err(ReplaceAttachmentError::Attach)?;
        self.attach_session_after_validation(connection, new_session_id.clone(), calling_options)
            .map_err(ReplaceAttachmentError::Attach)?;
        let detached_connections = self.detach_all_for_session(old_session_id);
        let cancelled_requests = self.cancel_session_server_requests(old_session_id);
        Ok(ReplaceAttachmentOutcome {
            old_session_id: old_session_id.clone(),
            new_session_id,
            calling_connection: connection,
            detached_connections,
            cancelled_requests,
        })
    }

    pub fn close_session_attachments(
        &mut self,
        session_id: &SessionId,
    ) -> CloseSessionAttachmentsOutcome {
        let outcome = CloseSessionAttachmentsOutcome {
            detached_connections: self.detach_all_for_session(session_id),
            cancelled_requests: self.cancel_session_server_requests(session_id),
        };
        self.rings.remove(session_id);
        outcome
    }

    pub fn disconnect(&mut self, connection: ConnectionKey) -> DisconnectOutcome {
        self.connection_senders.remove(&connection);
        self.request_senders.remove(&connection);
        self.lifecycle_senders.remove(&connection);
        self.callback_owners.retain(|_, owners| {
            owners.retain(|owner| *owner != connection);
            !owners.is_empty()
        });
        self.grants.retain(|(owner, _), _| *owner != connection);
        let sessions = self
            .connection_to_sessions
            .remove(&connection)
            .unwrap_or_default();
        for session_id in &sessions {
            self.attachments.remove(&(connection, session_id.clone()));
            if let Some(connections) = self.session_to_connections.get_mut(session_id) {
                connections.remove(&connection);
                if connections.is_empty() {
                    self.session_to_connections.remove(session_id);
                }
            }
        }
        let targeted: Vec<RequestId> = self
            .pending_server_requests
            .iter()
            .filter(|(_, entry)| entry.audience == ServerRequestAudience::Connection(connection))
            .map(|(request_id, _)| request_id.clone())
            .collect();
        for request_id in &targeted {
            self.remove_pending_server_request(request_id);
        }
        for entry in self.pending_server_requests.values_mut() {
            entry.recipients.remove(&connection);
        }
        // This may run inside another routing mutation (slow-consumer
        // disconnect during route/publish/notify), where the caller discards
        // the outcome. Record the cancelled ids so the owning `AppServer`
        // wrapper can still resolve their reply waiters.
        self.orphaned_waiter_requests
            .extend(targeted.iter().cloned());
        DisconnectOutcome {
            detached_sessions: sessions.into_iter().collect(),
            cancelled_requests: targeted,
        }
    }

    pub fn detach_session_for_connection(
        &mut self,
        connection: ConnectionKey,
        session_id: &SessionId,
    ) -> DetachSessionOutcome {
        let detached_attachment = self
            .attachments
            .remove(&(connection, session_id.clone()))
            .is_some();
        let revoked_grant = self
            .grants
            .remove(&(connection, session_id.clone()))
            .is_some();
        if detached_attachment {
            if let Some(sessions) = self.connection_to_sessions.get_mut(&connection) {
                sessions.remove(session_id);
            }
            if let Some(connections) = self.session_to_connections.get_mut(session_id) {
                connections.remove(&connection);
                if connections.is_empty() {
                    self.session_to_connections.remove(session_id);
                }
            }
        }
        self.callback_owners
            .retain(|(owned_session_id, _), owners| {
                if owned_session_id == session_id {
                    owners.retain(|owner| *owner != connection);
                }
                !owners.is_empty()
            });
        let cancelled_requests: Vec<_> = self
            .pending_server_requests
            .iter()
            .filter(|(_, entry)| {
                entry.pending.session_id == *session_id
                    && entry.audience == ServerRequestAudience::Connection(connection)
            })
            .map(|(request_id, _)| request_id.clone())
            .collect();
        for request_id in &cancelled_requests {
            self.remove_pending_server_request(request_id);
        }
        for entry in self.pending_server_requests.values_mut() {
            if entry.pending.session_id == *session_id {
                entry.recipients.remove(&connection);
            }
        }
        DetachSessionOutcome {
            detached: detached_attachment || revoked_grant,
            cancelled_requests,
        }
    }

    pub fn attachment(
        &self,
        connection: ConnectionKey,
        session_id: &SessionId,
    ) -> Option<&SessionAttachment> {
        self.attachments.get(&(connection, session_id.clone()))
    }

    pub fn grant(
        &self,
        connection: ConnectionKey,
        session_id: &SessionId,
    ) -> Option<&SessionGrant> {
        self.grants.get(&(connection, session_id.clone()))
    }

    pub fn session_access(
        &self,
        connection: ConnectionKey,
        session_id: &SessionId,
    ) -> Option<SessionAccess> {
        self.grant(connection, session_id).map(|grant| grant.access)
    }

    pub fn revoke_session_grants(&mut self, session_id: &SessionId) {
        self.grants
            .retain(|(_, granted_session_id), _| granted_session_id != session_id);
    }

    pub fn require_access(
        &self,
        connection: ConnectionKey,
        session_id: &SessionId,
        required_access: SessionAccess,
    ) -> Result<SessionGrant, SessionAccessError> {
        let Some(grant) = self.grant(connection, session_id) else {
            return Err(SessionAccessError::MissingGrant {
                connection,
                session_id: session_id.clone(),
            });
        };
        if required_access == SessionAccess::Full && grant.access == SessionAccess::ReadOnly {
            return Err(SessionAccessError::ReadOnly {
                connection,
                session_id: session_id.clone(),
            });
        }
        Ok(grant.clone())
    }

    pub fn require_full(
        &self,
        connection: ConnectionKey,
        session_id: &SessionId,
    ) -> Result<(), SessionAccessError> {
        self.require_access(connection, session_id, SessionAccess::Full)
            .map(|_| ())
    }

    pub fn connection_session_ids(&self, connection: ConnectionKey) -> HashSet<SessionId> {
        self.connection_to_sessions
            .get(&connection)
            .cloned()
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn connection_session_count(&self, connection: ConnectionKey) -> usize {
        self.connection_to_sessions
            .get(&connection)
            .map_or(0, HashSet::len)
    }

    pub fn register_connection_callback(
        &mut self,
        connection: ConnectionKey,
        session_id: SessionId,
        callback: ConnectionCallback,
    ) -> Result<(), SessionAccessError> {
        self.require_full(connection, &session_id)?;
        if !self
            .attachments
            .contains_key(&(connection, session_id.clone()))
        {
            return Err(SessionAccessError::NotAttached {
                connection,
                session_id,
            });
        }
        let owners = self
            .callback_owners
            .entry((session_id, callback))
            .or_default();
        owners.retain(|owner| *owner != connection);
        owners.insert(0, connection);
        Ok(())
    }

    pub fn connection_callback_owner(
        &self,
        session_id: &SessionId,
        callback: &ConnectionCallback,
    ) -> Option<ConnectionKey> {
        self.callback_owners
            .get(&(session_id.clone(), callback.clone()))
            .and_then(|owners| owners.first())
            .copied()
    }

    pub fn connection_count(&self) -> usize {
        self.connection_to_sessions.len()
    }

    pub fn connection_counts_for_session(&self, session_id: &SessionId) -> SessionConnectionCounts {
        let mut counts = SessionConnectionCounts::default();
        for connection in self
            .session_to_connections
            .get(session_id)
            .into_iter()
            .flatten()
        {
            match self
                .grants
                .get(&(*connection, session_id.clone()))
                .map(|grant| grant.access)
            {
                Some(SessionAccess::Full) => counts.full += 1,
                Some(SessionAccess::ReadOnly) => counts.read_only += 1,
                None => {}
            }
        }
        counts
    }

    pub fn initialize_ring_watermark(&mut self, session_id: SessionId, high_seq: i64) {
        self.ring_for(session_id).seed_high_seq(high_seq);
    }

    fn detach_all_for_session(&mut self, session_id: &SessionId) -> Vec<ConnectionKey> {
        self.callback_owners
            .retain(|(owned_session_id, _), _| owned_session_id != session_id);
        let connections = self
            .session_to_connections
            .remove(session_id)
            .unwrap_or_default();
        for connection in &connections {
            self.attachments.remove(&(*connection, session_id.clone()));
            if let Some(sessions) = self.connection_to_sessions.get_mut(connection) {
                sessions.remove(session_id);
            }
        }
        connections.into_iter().collect()
    }

    fn ring_for(&mut self, session_id: SessionId) -> &mut RetentionRing {
        self.rings
            .entry(session_id)
            .or_insert_with(|| RetentionRing::new(self.retention_per_session))
    }

    fn new_pending_server_request(
        session_id: SessionId,
        turn_id: Option<TurnId>,
    ) -> PendingServerRequest {
        let minted = NEXT_SERVER_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
        PendingServerRequest {
            request_id: RequestId::String(format!("server-request-{minted}")),
            session_id,
            turn_id,
            minted,
        }
    }

    fn insert_pending_server_request(&mut self, entry: PendingServerRequestEntry) {
        let pending = &entry.pending;
        self.pending_requests_by_session
            .entry(pending.session_id.clone())
            .or_default()
            .insert(pending.request_id.clone());
        if let Some(turn_id) = &pending.turn_id {
            self.pending_requests_by_turn
                .entry(turn_id.clone())
                .or_default()
                .insert(pending.request_id.clone());
        }
        self.pending_server_requests
            .insert(pending.request_id.clone(), entry);
    }

    pub(crate) fn remove_pending_server_request(
        &mut self,
        request_id: &RequestId,
    ) -> Option<PendingServerRequest> {
        let pending = self.pending_server_requests.remove(request_id)?.pending;
        if let Some(requests) = self
            .pending_requests_by_session
            .get_mut(&pending.session_id)
        {
            requests.remove(request_id);
            if requests.is_empty() {
                self.pending_requests_by_session.remove(&pending.session_id);
            }
        }
        if let Some(turn_id) = &pending.turn_id
            && let Some(requests) = self.pending_requests_by_turn.get_mut(turn_id)
        {
            requests.remove(request_id);
            if requests.is_empty() {
                self.pending_requests_by_turn.remove(turn_id);
            }
        }
        Some(pending)
    }

    fn cancel_session_server_requests(&mut self, session_id: &SessionId) -> Vec<RequestId> {
        let Some(requests) = self.pending_requests_by_session.remove(session_id) else {
            return Vec::new();
        };
        requests
            .into_iter()
            .filter(|request_id| {
                self.cancel_pending_server_request(request_id, "owning session ended")
                    .is_some()
            })
            .collect()
    }

    /// Pending broadcast ids for `session_id`, oldest first (`minted` order),
    /// so an attach-time replay re-delivers prompts in the order they were
    /// asked.
    fn pending_broadcast_request_ids_for_session(&self, session_id: &SessionId) -> Vec<RequestId> {
        let mut requests: Vec<(i64, RequestId)> = self
            .pending_requests_by_session
            .get(session_id)
            .into_iter()
            .flatten()
            .filter_map(|request_id| {
                let entry = self.pending_server_requests.get(request_id)?;
                (entry.audience == ServerRequestAudience::AllFullConnections)
                    .then(|| (entry.pending.minted, request_id.clone()))
            })
            .collect();
        requests.sort_by_key(|(minted, _)| *minted);
        requests.into_iter().map(|(_, id)| id).collect()
    }

    fn notify_server_request_losers(
        &mut self,
        session_id: &SessionId,
        request_id: &RequestId,
        losing_connections: Vec<ConnectionKey>,
        reason: &str,
    ) {
        let cancellation = ServerRequest::CancelRequest(ServerCancelRequestParams {
            request_id: request_id.as_display(),
            reason: Some(reason.to_string()),
        });
        for connection in losing_connections {
            let Some(sender) = self.request_senders.get(&connection).cloned() else {
                continue;
            };
            if sender
                .try_send(ServerRequestDelivery {
                    session_id: session_id.clone(),
                    request_id: request_id.clone(),
                    request: cancellation.clone(),
                })
                .is_err()
            {
                self.disconnect(connection);
            }
        }
    }

    fn cancel_pending_server_request(
        &mut self,
        request_id: &RequestId,
        reason: &str,
    ) -> Option<PendingServerRequest> {
        let entry = self.pending_server_requests.get(request_id)?;
        let session_id = entry.pending.session_id.clone();
        let recipients = entry.recipients.iter().copied().collect();
        let pending = self.remove_pending_server_request(request_id)?;
        self.notify_server_request_losers(&session_id, request_id, recipients, reason);
        Some(pending)
    }

    pub(crate) fn expire_server_request(&mut self, request_id: &RequestId) -> bool {
        self.cancel_pending_server_request(request_id, "server request timed out")
            .is_some()
    }
}

/// Reply kind expected for a pending request. `None` for `CancelRequest`,
/// which `prepare_server_request` rejects, so no pending entry ever carries
/// it.
fn server_request_reply_kind(request: &ServerRequest) -> Option<ServerRequestReplyKind> {
    match request {
        ServerRequest::AskForApproval(_) => Some(ServerRequestReplyKind::Approval),
        ServerRequest::RequestUserInput(_) => Some(ServerRequestReplyKind::UserInput),
        ServerRequest::RequestElicitation(_) => Some(ServerRequestReplyKind::Elicitation),
        ServerRequest::McpRouteMessage(_) => Some(ServerRequestReplyKind::McpRouteMessage),
        ServerRequest::HookCallback(_) => Some(ServerRequestReplyKind::HookCallback),
        ServerRequest::CancelRequest(_) => None,
    }
}

#[derive(Debug)]
struct RetentionRing {
    capacity: usize,
    envelopes: VecDeque<SessionEnvelope>,
    high_seq: Option<i64>,
}

impl RetentionRing {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            envelopes: VecDeque::new(),
            high_seq: None,
        }
    }

    fn seed_high_seq(&mut self, seq: i64) {
        self.high_seq = Some(self.high_seq.map_or(seq, |current| current.max(seq)));
    }

    fn append(&mut self, envelope: SessionEnvelope) {
        if let Some(seq) = envelope.session_seq {
            self.high_seq = Some(self.high_seq.map_or(seq, |current| current.max(seq)));
        }
        if self.capacity == 0 {
            self.envelopes.clear();
            return;
        }
        self.envelopes.push_back(envelope);
        while self.envelopes.len() > self.capacity {
            self.envelopes.pop_front();
        }
    }

    fn replay_after(&self, after_seq: i64) -> RingReplay {
        match self.envelopes.front().and_then(|env| env.session_seq) {
            Some(oldest_seq) => {
                if after_seq < oldest_seq - 1 {
                    return RingReplay::TooOld;
                }
                RingReplay::Available(
                    self.envelopes
                        .iter()
                        .filter(|env| env.session_seq.is_some_and(|seq| seq > after_seq))
                        .cloned()
                        .collect(),
                )
            }
            None => match self.high_seq {
                Some(high) if after_seq >= high => RingReplay::Available(Vec::new()),
                Some(_) => RingReplay::TooOld,
                None => RingReplay::Available(Vec::new()),
            },
        }
    }
}

#[derive(Debug)]
enum RingReplay {
    Available(Vec<SessionEnvelope>),
    TooOld,
}

#[cfg(test)]
#[path = "routing.test.rs"]
mod tests;
