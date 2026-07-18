//! App-server lifecycle, routing, and protocol adapters.
//!
//! The crate owns opaque live-session slots, connection/surface indexes,
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
    InteractiveTarget, RequestId, ServerRequest, ServerRequestDelivery, SessionEnvelope, SessionId,
    SurfaceDelivery, SurfaceId, SurfaceLifecycleEffect, SurfaceLifecycleEffectKind, TurnId,
};
use snafu::Snafu;

pub use activity::SessionActivityTracker;
pub use app_server::{
    AppCloseCommit, AppCloseStart, AppLiveSessionSummary, AppLoadStart, AppReplaceCommit,
    AppReplaceStart, AppServer, AppServerError, AppShutdownSession, AppShutdownStart,
    ResolvedServerRequest, ServerRequestErrorReply, ServerRequestReply,
    ValidatedInteractiveSession,
};
pub use json_rpc_adapter::{
    JsonRpcAdapter, JsonRpcAdapterConnection, JsonRpcAdapterError, JsonRpcConnectionHandlerFactory,
    JsonRpcConnectionOwnerError, JsonRpcDispatchError, JsonRpcRequestContext, JsonRpcRequestFuture,
    JsonRpcRequestHandler, JsonRpcServerRequestResponse, PendingJsonRpcServerRequest,
};
pub use local_client_adapter::{
    LocalClientAdapter, LocalClientConnection, LocalClientDispatchError, LocalClientInbound,
    LocalClientRequestContext, LocalClientRequestFuture, LocalClientRequestHandler,
    LocalClientSubscribeOutcome, LocalClientSubscription, LocalClientSurface,
};
pub use registration_policy::{
    SessionEgress, SessionRegistrationPolicy, SessionTopology, SessionVisibility,
};
pub use registry::{
    CloseCompletion, CloseStart, LiveSessionRegistry, LoadCompletion, LoadStart, RegistryError,
    ReplaceCommit, ReplaceStart,
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
///
/// The inner value is intentionally not serializable and never appears on the
/// wire or on disk. Public construction is server-owned via [`Self::generate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConnectionKey(i64);

impl ConnectionKey {
    pub fn generate() -> Self {
        Self(NEXT_CONNECTION_KEY.fetch_add(1, Ordering::Relaxed))
    }

    #[cfg(test)]
    fn for_test(id: i64) -> Self {
        Self(id)
    }
}

pub type OutboundSender = tokio::sync::mpsc::Sender<SurfaceDelivery>;

pub type ServerRequestSender = tokio::sync::mpsc::Sender<ServerRequestDelivery>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceRole {
    Interactive,
    Passive,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SurfaceCapabilities {
    pub file_picker: bool,
    pub keychain: bool,
    pub attestation: bool,
    pub notifications: bool,
}

impl SurfaceCapabilities {
    pub fn includes(self, capability: SurfaceCapability) -> bool {
        match capability {
            SurfaceCapability::Interactive => true,
            SurfaceCapability::FilePicker => self.file_picker,
            SurfaceCapability::Keychain => self.keychain,
            SurfaceCapability::Attestation => self.attestation,
            SurfaceCapability::Notifications => self.notifications,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SurfaceCapability {
    Interactive,
    FilePicker,
    Keychain,
    Attestation,
    Notifications,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceState {
    Attached,
    SessionClosed,
}

#[derive(Debug, Clone)]
pub struct SurfaceAttachment {
    pub surface_id: SurfaceId,
    pub connection: ConnectionKey,
    pub session_id: SessionId,
    pub role: SurfaceRole,
    pub capabilities: SurfaceCapabilities,
    pub notification_prefs: NotificationPrefs,
    pub last_delivered_seq: i64,
    pub state: SurfaceState,
    pub attached_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceLimits {
    pub max_surfaces_per_connection: usize,
    pub max_passive_surfaces_per_session: usize,
}

impl Default for SurfaceLimits {
    fn default() -> Self {
        Self {
            max_surfaces_per_connection: 8,
            max_passive_surfaces_per_session: 16,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AttachSurfaceOptions {
    pub role: SurfaceRole,
    pub capabilities: SurfaceCapabilities,
    pub notification_prefs: NotificationPrefs,
    pub last_delivered_seq: i64,
}

impl Default for AttachSurfaceOptions {
    fn default() -> Self {
        Self {
            role: SurfaceRole::Passive,
            capabilities: SurfaceCapabilities::default(),
            notification_prefs: NotificationPrefs::default(),
            last_delivered_seq: 0,
        }
    }
}

#[stack_trace_debug]
#[derive(Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum AttachError {
    #[snafu(display("session {session_id} already has an interactive surface"))]
    InteractiveOwnerConflict {
        session_id: SessionId,
        owner_surface: SurfaceId,
        owner_attached_at: DateTime<Utc>,
        owner_idle: bool,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("surface limit reached"))]
    SurfaceLimit {
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
}

impl ErrorExt for AttachError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::InteractiveOwnerConflict { .. } => StatusCode::InvalidArguments,
            Self::SurfaceLimit { .. } => StatusCode::ResourcesExhausted,
            Self::SessionClosing { .. } => StatusCode::Cancelled,
            Self::SessionNotFound { .. } => StatusCode::FileNotFound,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Debug, Clone)]
pub enum SubscribeReplay {
    Replayed(Vec<SessionEnvelope>),
    SnapshotRequired,
}

/// Result of routing one envelope through the current surface map.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteOutcome {
    pub delivered: usize,
    pub disconnected: Vec<ConnectionKey>,
    /// Server-request ids cancelled by disconnecting a full/closed connection
    /// mid-route. The AppServer layer must resolve these waiters after the
    /// routing lock is released; dropping them would wedge an in-flight turn.
    pub cancelled_requests: Vec<RequestId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DisconnectOutcome {
    pub detached_surfaces: Vec<SurfaceId>,
    pub cancelled_requests: Vec<RequestId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DetachSurfaceOutcome {
    pub detached_surface: Option<SurfaceId>,
    pub cancelled_requests: Vec<RequestId>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionSurfaceCounts {
    pub attached: usize,
    pub closed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceSurfaceOutcome {
    pub old_session_id: SessionId,
    pub new_session_id: SessionId,
    pub calling_surface: SurfaceId,
    pub detached_surfaces: Vec<SurfaceId>,
    pub cancelled_requests: Vec<RequestId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CloseSessionSurfacesOutcome {
    pub closed_surfaces: Vec<SurfaceId>,
    pub cancelled_requests: Vec<RequestId>,
}

pub type SurfaceLifecycleSender = tokio::sync::mpsc::Sender<SurfaceLifecycleEffect>;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LifecycleRouteOutcome {
    pub delivered: usize,
    pub disconnected: Vec<ConnectionKey>,
    /// Server-request ids cancelled by disconnecting a full/closed connection
    /// mid-route. See [`RouteOutcome::cancelled_requests`].
    pub cancelled_requests: Vec<RequestId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingServerRequest {
    pub request_id: RequestId,
    pub session_id: SessionId,
    pub surface_id: SurfaceId,
    pub capability: SurfaceCapability,
    pub turn_id: Option<TurnId>,
    /// Monotonic mint order, used to replay pending requests in the order
    /// they were opened. Sorting by the string `request_id` would order
    /// "server-request-10" before "server-request-2".
    pub minted: i64,
}

#[derive(Debug, Clone)]
pub struct PendingServerRequestReplay {
    pub pending: PendingServerRequest,
    pub request: ServerRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenServerRequestError {
    NoInteractiveSurface {
        session_id: SessionId,
        capability: SurfaceCapability,
    },
    CapabilityNotDeclared {
        session_id: SessionId,
        surface_id: SurfaceId,
        capability: SurfaceCapability,
    },
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
    WrongSurface {
        request_id: RequestId,
        expected_surface_id: SurfaceId,
        actual_surface_id: SurfaceId,
    },
    WrongConnection {
        request_id: RequestId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerRequestRouteOutcome {
    pub pending: PendingServerRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerRequestRouteError {
    NoInteractiveSurface {
        session_id: SessionId,
        capability: SurfaceCapability,
    },
    CapabilityNotDeclared {
        session_id: SessionId,
        surface_id: SurfaceId,
        capability: SurfaceCapability,
    },
    NoRequestChannel {
        surface_id: SurfaceId,
    },
    QueueUnavailable {
        request_id: RequestId,
        surface_id: SurfaceId,
        /// Other pending server-request ids cancelled by disconnecting the
        /// full/closed connection. The AppServer layer must resolve their
        /// waiters even though this route failed.
        cancelled_requests: Vec<RequestId>,
    },
}

impl From<OpenServerRequestError> for ServerRequestRouteError {
    fn from(error: OpenServerRequestError) -> Self {
        match error {
            OpenServerRequestError::NoInteractiveSurface {
                session_id,
                capability,
            } => Self::NoInteractiveSurface {
                session_id,
                capability,
            },
            OpenServerRequestError::CapabilityNotDeclared {
                session_id,
                surface_id,
                capability,
            } => Self::CapabilityNotDeclared {
                session_id,
                surface_id,
                capability,
            },
        }
    }
}

/// Single-lock routing state for connections, surfaces, and replay rings.
#[derive(Debug)]
pub struct RoutingState {
    retention_per_session: usize,
    limits: SurfaceLimits,
    attachments: HashMap<SurfaceId, SurfaceAttachment>,
    surface_to_session: HashMap<SurfaceId, SessionId>,
    session_to_surfaces: HashMap<SessionId, HashSet<SurfaceId>>,
    surface_to_connection: HashMap<SurfaceId, ConnectionKey>,
    connection_to_surfaces: HashMap<ConnectionKey, HashSet<SurfaceId>>,
    interactive_owners: HashMap<SessionId, SurfaceId>,
    connection_senders: HashMap<ConnectionKey, OutboundSender>,
    request_senders: HashMap<ConnectionKey, ServerRequestSender>,
    lifecycle_senders: HashMap<ConnectionKey, SurfaceLifecycleSender>,
    rings: HashMap<SessionId, RetentionRing>,
    pending_server_requests: HashMap<RequestId, PendingServerRequest>,
    pending_server_request_payloads: HashMap<RequestId, ServerRequest>,
    pending_requests_by_session: HashMap<SessionId, HashSet<RequestId>>,
    pending_requests_by_surface: HashMap<SurfaceId, HashSet<RequestId>>,
    pending_requests_by_turn: HashMap<TurnId, HashSet<RequestId>>,
}

impl RoutingState {
    pub fn new(retention_per_session: usize) -> Self {
        Self::new_with_limits(retention_per_session, SurfaceLimits::default())
    }

    pub fn new_with_limits(retention_per_session: usize, limits: SurfaceLimits) -> Self {
        Self {
            retention_per_session,
            limits,
            attachments: HashMap::new(),
            surface_to_session: HashMap::new(),
            session_to_surfaces: HashMap::new(),
            surface_to_connection: HashMap::new(),
            connection_to_surfaces: HashMap::new(),
            interactive_owners: HashMap::new(),
            connection_senders: HashMap::new(),
            request_senders: HashMap::new(),
            lifecycle_senders: HashMap::new(),
            rings: HashMap::new(),
            pending_server_requests: HashMap::new(),
            pending_server_request_payloads: HashMap::new(),
            pending_requests_by_session: HashMap::new(),
            pending_requests_by_surface: HashMap::new(),
            pending_requests_by_turn: HashMap::new(),
        }
    }

    pub fn connect(&mut self, connection: ConnectionKey, sender: OutboundSender) {
        self.connection_senders.insert(connection, sender);
        self.request_senders.remove(&connection);
        self.lifecycle_senders.remove(&connection);
        self.connection_to_surfaces.entry(connection).or_default();
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
        lifecycle_sender: SurfaceLifecycleSender,
    ) {
        self.connect(connection, sender);
        self.lifecycle_senders.insert(connection, lifecycle_sender);
    }

    pub fn connect_with_request_and_lifecycle_senders(
        &mut self,
        connection: ConnectionKey,
        sender: OutboundSender,
        request_sender: ServerRequestSender,
        lifecycle_sender: SurfaceLifecycleSender,
    ) {
        self.connect_with_request_sender(connection, sender, request_sender);
        self.lifecycle_senders.insert(connection, lifecycle_sender);
    }

    /// Register a surface without replay. Used after a caller has already
    /// established its baseline through `session/read`.
    pub fn attach_surface(
        &mut self,
        connection: ConnectionKey,
        surface_id: SurfaceId,
        session_id: SessionId,
    ) -> Result<(), AttachError> {
        self.attach_surface_with_options(
            connection,
            surface_id,
            session_id,
            AttachSurfaceOptions::default(),
        )
    }

    pub fn attach_surface_with_options(
        &mut self,
        connection: ConnectionKey,
        surface_id: SurfaceId,
        session_id: SessionId,
        options: AttachSurfaceOptions,
    ) -> Result<(), AttachError> {
        self.validate_attach(connection, &surface_id, &session_id, options.role)?;
        self.detach_surface(&surface_id);
        let attachment = SurfaceAttachment {
            surface_id: surface_id.clone(),
            connection,
            session_id: session_id.clone(),
            role: options.role,
            capabilities: options.capabilities,
            notification_prefs: options.notification_prefs,
            last_delivered_seq: options.last_delivered_seq,
            state: SurfaceState::Attached,
            attached_at: Utc::now(),
        };
        if options.role == SurfaceRole::Interactive {
            self.interactive_owners
                .insert(session_id.clone(), surface_id.clone());
        }
        self.attachments.insert(surface_id.clone(), attachment);
        self.surface_to_session
            .insert(surface_id.clone(), session_id.clone());
        self.session_to_surfaces
            .entry(session_id)
            .or_default()
            .insert(surface_id.clone());
        self.surface_to_connection
            .insert(surface_id.clone(), connection);
        self.connection_to_surfaces
            .entry(connection)
            .or_default()
            .insert(surface_id);
        Ok(())
    }

    /// Replay durable envelopes after `after_seq`, then register the surface.
    ///
    /// Replay lookup and registration happen in this single mutable method so a
    /// caller can place `RoutingState` behind one `std::sync::RwLock` and keep
    /// the replay->live transition atomic.
    pub fn subscribe(
        &mut self,
        connection: ConnectionKey,
        surface_id: SurfaceId,
        session_id: SessionId,
        after_seq: Option<i64>,
    ) -> Result<SubscribeReplay, AttachError> {
        self.subscribe_with_options(
            connection,
            surface_id,
            session_id,
            after_seq,
            AttachSurfaceOptions::default(),
        )
    }

    pub fn subscribe_with_options(
        &mut self,
        connection: ConnectionKey,
        surface_id: SurfaceId,
        session_id: SessionId,
        after_seq: Option<i64>,
        options: AttachSurfaceOptions,
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
                self.attach_surface_with_options(connection, surface_id, session_id, options)?;
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

        let Some(surfaces) = self.session_to_surfaces.get(&envelope.session_id) else {
            return RouteOutcome::default();
        };
        let deliveries: Vec<(SurfaceId, ConnectionKey, OutboundSender)> = surfaces
            .iter()
            .filter_map(|surface_id| {
                let attachment = self.attachments.get(surface_id)?;
                if !attachment.notification_prefs.accepts(&envelope) {
                    return None;
                }
                let connection = *self.surface_to_connection.get(surface_id)?;
                let sender = self.connection_senders.get(&connection)?.clone();
                Some((surface_id.clone(), connection, sender))
            })
            .collect();

        let mut outcome = RouteOutcome::default();
        for (surface_id, connection, sender) in deliveries {
            let delivery = SurfaceDelivery {
                surface_id: surface_id.clone(),
                envelope: envelope.clone(),
            };
            match sender.try_send(delivery) {
                Ok(()) => {
                    outcome.delivered += 1;
                    // Track the per-surface delivery cursor.
                    if let Some(seq) = envelope.session_seq
                        && let Some(attachment) = self.attachments.get_mut(&surface_id)
                    {
                        attachment.last_delivered_seq = seq;
                    }
                }
                Err(tokio::sync::mpsc::error::TrySendError::Full(_))
                | Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    if !outcome.disconnected.contains(&connection) {
                        outcome.disconnected.push(connection);
                        let disconnected = self.disconnect(connection);
                        outcome
                            .cancelled_requests
                            .extend(disconnected.cancelled_requests);
                    }
                }
            }
        }
        outcome
    }

    pub fn route_lifecycle_effects(
        &mut self,
        effects: Vec<SurfaceLifecycleEffect>,
    ) -> LifecycleRouteOutcome {
        let deliveries: Vec<(
            ConnectionKey,
            SurfaceLifecycleSender,
            SurfaceLifecycleEffect,
        )> = effects
            .into_iter()
            .filter_map(|effect| {
                let connection = *self.surface_to_connection.get(&effect.surface_id)?;
                let sender = self.lifecycle_senders.get(&connection)?.clone();
                Some((connection, sender, effect))
            })
            .collect();

        let mut outcome = LifecycleRouteOutcome::default();
        let mut purge: Vec<SurfaceId> = Vec::new();
        for (connection, sender, effect) in deliveries {
            if outcome.disconnected.contains(&connection) {
                continue;
            }
            let surface_id = effect.surface_id.clone();
            let terminal = matches!(
                effect.kind,
                SurfaceLifecycleEffectKind::SessionEnded { .. }
                    | SurfaceLifecycleEffectKind::SessionReplaced { .. }
            );
            match sender.try_send(effect) {
                Ok(()) => {
                    outcome.delivered += 1;
                    if terminal {
                        purge.push(surface_id);
                    }
                }
                Err(tokio::sync::mpsc::error::TrySendError::Full(_))
                | Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    outcome.disconnected.push(connection);
                    let disconnected = self.disconnect(connection);
                    outcome
                        .cancelled_requests
                        .extend(disconnected.cancelled_requests);
                }
            }
        }
        // A closed surface's routing metadata is only needed long enough to
        // deliver its terminal effect; drop it now so a long-lived connection
        // does not accumulate `SessionClosed` attachments without bound.
        for surface_id in purge {
            if self
                .attachments
                .get(&surface_id)
                .is_some_and(|attachment| attachment.state == SurfaceState::SessionClosed)
            {
                self.purge_closed_surface(&surface_id);
            }
        }
        outcome
    }

    /// Remove the leftover routing metadata for a surface whose session has
    /// closed and whose terminal lifecycle effect has already been delivered.
    /// `close_surface` intentionally keeps this metadata alive for delivery;
    /// this is the paired cleanup.
    fn purge_closed_surface(&mut self, surface_id: &SurfaceId) {
        self.attachments.remove(surface_id);
        if let Some(connection) = self.surface_to_connection.remove(surface_id)
            && let Some(surfaces) = self.connection_to_surfaces.get_mut(&connection)
        {
            surfaces.remove(surface_id);
        }
    }

    /// Mint and record a server->client request for the interactive surface.
    ///
    /// This crate tracks request ownership and cancellation only; the transport
    /// adapter sends the actual protocol request and stores its reply channel.
    pub fn open_server_request(
        &mut self,
        session_id: SessionId,
        capability: SurfaceCapability,
        turn_id: Option<TurnId>,
    ) -> Result<PendingServerRequest, OpenServerRequestError> {
        let surface_id = self.server_request_target(&session_id, capability)?;
        let pending = Self::new_pending_server_request(session_id, surface_id, capability, turn_id);
        self.insert_pending_server_request(pending.clone());
        Ok(pending)
    }

    pub fn route_server_request(
        &mut self,
        session_id: SessionId,
        capability: SurfaceCapability,
        turn_id: Option<TurnId>,
        request: ServerRequest,
    ) -> Result<ServerRequestRouteOutcome, ServerRequestRouteError> {
        let surface_id = self
            .server_request_target(&session_id, capability)
            .map_err(ServerRequestRouteError::from)?;
        let connection = self
            .surface_to_connection
            .get(&surface_id)
            .copied()
            .ok_or_else(|| ServerRequestRouteError::NoRequestChannel {
                surface_id: surface_id.clone(),
            })?;
        let Some(sender) = self.request_senders.get(&connection).cloned() else {
            return Err(ServerRequestRouteError::NoRequestChannel { surface_id });
        };

        let pending =
            Self::new_pending_server_request(session_id, surface_id.clone(), capability, turn_id);
        self.insert_pending_server_request(pending.clone());
        self.pending_server_request_payloads
            .insert(pending.request_id.clone(), request.clone());
        let delivery = ServerRequestDelivery {
            session_id: pending.session_id.clone(),
            surface_id: surface_id.clone(),
            request_id: pending.request_id.clone(),
            request,
        };
        match sender.try_send(delivery) {
            Ok(()) => Ok(ServerRequestRouteOutcome { pending }),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_))
            | Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                let request_id = pending.request_id;
                let disconnected = self.disconnect(connection);
                Err(ServerRequestRouteError::QueueUnavailable {
                    request_id,
                    surface_id,
                    cancelled_requests: disconnected.cancelled_requests,
                })
            }
        }
    }

    pub fn complete_server_request(
        &mut self,
        request_id: &RequestId,
        target: &InteractiveTarget,
    ) -> Result<PendingServerRequest, CompleteServerRequestError> {
        let Some(pending) = self.pending_server_requests.get(request_id) else {
            return Err(CompleteServerRequestError::NotFound {
                request_id: request_id.clone(),
            });
        };
        if pending.session_id != target.session_id {
            return Err(CompleteServerRequestError::WrongSession {
                request_id: request_id.clone(),
                expected_session_id: pending.session_id.clone(),
                actual_session_id: target.session_id.clone(),
            });
        }
        if pending.surface_id != target.surface_id {
            return Err(CompleteServerRequestError::WrongSurface {
                request_id: request_id.clone(),
                expected_surface_id: pending.surface_id.clone(),
                actual_surface_id: target.surface_id.clone(),
            });
        }
        self.remove_pending_server_request(request_id)
            .ok_or_else(|| CompleteServerRequestError::NotFound {
                request_id: request_id.clone(),
            })
    }

    pub fn complete_server_request_by_id(
        &mut self,
        request_id: &RequestId,
    ) -> Result<PendingServerRequest, CompleteServerRequestError> {
        self.remove_pending_server_request(request_id)
            .ok_or_else(|| CompleteServerRequestError::NotFound {
                request_id: request_id.clone(),
            })
    }

    pub fn cancel_server_request_for_connection(
        &mut self,
        request_id: &RequestId,
        connection: ConnectionKey,
    ) -> Result<PendingServerRequest, CompleteServerRequestError> {
        let Some(pending) = self.pending_server_requests.get(request_id) else {
            return Err(CompleteServerRequestError::NotFound {
                request_id: request_id.clone(),
            });
        };
        if self.surface_to_connection.get(&pending.surface_id) != Some(&connection) {
            return Err(CompleteServerRequestError::WrongConnection {
                request_id: request_id.clone(),
            });
        }
        self.remove_pending_server_request(request_id)
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
            .filter(|request_id| self.remove_pending_server_request(request_id).is_some())
            .collect()
    }

    pub fn pending_server_requests_for_surface(
        &self,
        surface_id: &SurfaceId,
    ) -> Vec<PendingServerRequest> {
        self.pending_requests_by_surface
            .get(surface_id)
            .into_iter()
            .flat_map(|requests| requests.iter())
            .filter_map(|request_id| self.pending_server_requests.get(request_id).cloned())
            .collect()
    }

    pub fn pending_server_request_replays_for_surface(
        &self,
        surface_id: &SurfaceId,
    ) -> Vec<PendingServerRequestReplay> {
        let mut replays = self
            .pending_requests_by_surface
            .get(surface_id)
            .into_iter()
            .flat_map(|requests| requests.iter())
            .filter_map(|request_id| {
                let pending = self.pending_server_requests.get(request_id)?.clone();
                let request = self
                    .pending_server_request_payloads
                    .get(request_id)?
                    .clone();
                Some(PendingServerRequestReplay { pending, request })
            })
            .collect::<Vec<_>>();
        replays.sort_by_key(|replay| replay.pending.minted);
        replays
    }

    /// Re-point the calling surface to a replacement session and close peers.
    ///
    /// This is the `RoutingState` half of the replace commit. Callers must invoke it
    /// while holding the app-server routing lock, in the same no-await commit
    /// section that swaps registry slots.
    pub fn replace_calling_surface(
        &mut self,
        calling_surface: &SurfaceId,
        new_session_id: SessionId,
    ) -> Option<ReplaceSurfaceOutcome> {
        let old_session_id = self.surface_to_session.get(calling_surface)?.clone();
        let old_surfaces: Vec<SurfaceId> = self
            .session_to_surfaces
            .get(&old_session_id)
            .into_iter()
            .flat_map(|surfaces| surfaces.iter().cloned())
            .collect();
        let cancelled_requests = self.cancel_session_server_requests(&old_session_id);
        let mut detached_surfaces = Vec::new();

        for surface_id in old_surfaces {
            if &surface_id == calling_surface {
                self.repoint_surface_to_session(calling_surface, new_session_id.clone());
            } else {
                self.close_surface(&surface_id);
                detached_surfaces.push(surface_id);
            }
        }

        Some(ReplaceSurfaceOutcome {
            old_session_id,
            new_session_id,
            calling_surface: calling_surface.clone(),
            detached_surfaces,
            cancelled_requests,
        })
    }

    /// Mark every surface on a session as closed and remove them from fan-out.
    pub fn close_session_surfaces(
        &mut self,
        session_id: &SessionId,
    ) -> CloseSessionSurfacesOutcome {
        let surfaces: Vec<SurfaceId> = self
            .session_to_surfaces
            .get(session_id)
            .into_iter()
            .flat_map(|surfaces| surfaces.iter().cloned())
            .collect();
        let mut cancelled_requests = self.cancel_session_server_requests(session_id);
        for surface_id in &surfaces {
            cancelled_requests.extend(self.close_surface(surface_id));
        }
        // The session is closing: its retention ring is dead history now, so
        // drop it rather than leaking it for the process lifetime.
        self.rings.remove(session_id);
        CloseSessionSurfacesOutcome {
            closed_surfaces: surfaces,
            cancelled_requests,
        }
    }

    pub fn disconnect(&mut self, connection: ConnectionKey) -> DisconnectOutcome {
        self.connection_senders.remove(&connection);
        self.request_senders.remove(&connection);
        self.lifecycle_senders.remove(&connection);
        let Some(surfaces) = self.connection_to_surfaces.remove(&connection) else {
            return DisconnectOutcome::default();
        };
        let detached_surfaces: Vec<SurfaceId> = surfaces.into_iter().collect();
        let mut cancelled_requests = Vec::new();
        for surface_id in &detached_surfaces {
            cancelled_requests.extend(self.detach_surface(surface_id));
        }
        DisconnectOutcome {
            detached_surfaces,
            cancelled_requests,
        }
    }

    pub fn detach_surface_for_connection(
        &mut self,
        connection: ConnectionKey,
        surface_id: &SurfaceId,
    ) -> DetachSurfaceOutcome {
        if self.surface_to_connection.get(surface_id) != Some(&connection) {
            return DetachSurfaceOutcome::default();
        }
        DetachSurfaceOutcome {
            detached_surface: Some(surface_id.clone()),
            cancelled_requests: self.detach_surface(surface_id),
        }
    }

    pub fn surface_session(&self, surface_id: &SurfaceId) -> Option<&SessionId> {
        self.surface_to_session.get(surface_id)
    }

    pub fn surface_attachment(&self, surface_id: &SurfaceId) -> Option<&SurfaceAttachment> {
        self.attachments.get(surface_id)
    }

    pub fn interactive_owner(&self, session_id: &SessionId) -> Option<&SurfaceId> {
        self.interactive_owners.get(session_id)
    }

    pub fn connection_session_ids(&self, connection: ConnectionKey) -> HashSet<SessionId> {
        self.connection_to_surfaces
            .get(&connection)
            .into_iter()
            .flat_map(|surfaces| surfaces.iter())
            .filter_map(|surface_id| self.surface_to_session.get(surface_id).cloned())
            .collect()
    }

    pub fn connection_surface_count(&self, connection: ConnectionKey) -> usize {
        self.connection_to_surfaces
            .get(&connection)
            .map_or(0, HashSet::len)
    }

    pub fn attached_connection_surface_count(&self, connection: ConnectionKey) -> usize {
        self.connection_to_surfaces
            .get(&connection)
            .into_iter()
            .flat_map(|surfaces| surfaces.iter())
            .filter(|surface_id| {
                self.attachments
                    .get(*surface_id)
                    .is_some_and(|attachment| attachment.state == SurfaceState::Attached)
            })
            .count()
    }

    pub fn surface_counts_for_session(&self, session_id: &SessionId) -> SessionSurfaceCounts {
        let mut counts = SessionSurfaceCounts::default();
        for attachment in self
            .attachments
            .values()
            .filter(|attachment| &attachment.session_id == session_id)
        {
            match attachment.state {
                SurfaceState::Attached => counts.attached += 1,
                SurfaceState::SessionClosed => counts.closed += 1,
            }
        }
        counts
    }

    fn ring_for(&mut self, session_id: SessionId) -> &mut RetentionRing {
        self.rings
            .entry(session_id)
            .or_insert_with(|| RetentionRing::new(self.retention_per_session))
    }

    /// Seed a session's ring high-water baseline from its resume skip-ahead
    ///, so a stale `after_seq` on the still-empty ring degrades to
    /// `snapshot_required` rather than a silent no-replay attach.
    pub fn initialize_ring_watermark(&mut self, session_id: SessionId, high_seq: i64) {
        self.ring_for(session_id).seed_high_seq(high_seq);
    }

    fn server_request_target(
        &self,
        session_id: &SessionId,
        capability: SurfaceCapability,
    ) -> Result<SurfaceId, OpenServerRequestError> {
        let Some(surface_id) = self.interactive_owners.get(session_id).cloned() else {
            return Err(OpenServerRequestError::NoInteractiveSurface {
                session_id: session_id.clone(),
                capability,
            });
        };
        let attachment = self
            .attachments
            .get(&surface_id)
            .filter(|attachment| attachment.state == SurfaceState::Attached)
            .ok_or_else(|| OpenServerRequestError::NoInteractiveSurface {
                session_id: session_id.clone(),
                capability,
            })?;
        if !attachment.capabilities.includes(capability) {
            return Err(OpenServerRequestError::CapabilityNotDeclared {
                session_id: session_id.clone(),
                surface_id,
                capability,
            });
        }
        Ok(surface_id)
    }

    fn new_pending_server_request(
        session_id: SessionId,
        surface_id: SurfaceId,
        capability: SurfaceCapability,
        turn_id: Option<TurnId>,
    ) -> PendingServerRequest {
        let minted = NEXT_SERVER_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
        PendingServerRequest {
            request_id: RequestId::String(format!("server-request-{minted}")),
            session_id,
            surface_id,
            capability,
            turn_id,
            minted,
        }
    }

    fn detach_surface(&mut self, surface_id: &SurfaceId) -> Vec<RequestId> {
        let cancelled_requests = self.cancel_surface_server_requests(surface_id);
        let attachment = self.attachments.remove(surface_id);
        if let Some(attachment) = &attachment
            && attachment.role == SurfaceRole::Interactive
        {
            self.interactive_owners.remove(&attachment.session_id);
        }
        let session_id = self.surface_to_session.remove(surface_id);
        if let Some(session_id) = session_id
            && let Some(surfaces) = self.session_to_surfaces.get_mut(&session_id)
        {
            surfaces.remove(surface_id);
            if surfaces.is_empty() {
                self.session_to_surfaces.remove(&session_id);
            }
        }
        let connection = self.surface_to_connection.remove(surface_id);
        if let Some(connection) = connection
            && let Some(surfaces) = self.connection_to_surfaces.get_mut(&connection)
        {
            surfaces.remove(surface_id);
        }
        cancelled_requests
    }

    fn close_surface(&mut self, surface_id: &SurfaceId) -> Vec<RequestId> {
        let cancelled_requests = self.cancel_surface_server_requests(surface_id);
        let session_id = self.surface_to_session.remove(surface_id);
        if let Some(session_id) = session_id
            && let Some(surfaces) = self.session_to_surfaces.get_mut(&session_id)
        {
            surfaces.remove(surface_id);
            if surfaces.is_empty() {
                self.session_to_surfaces.remove(&session_id);
            }
        }
        if let Some(attachment) = self.attachments.get_mut(surface_id) {
            if attachment.role == SurfaceRole::Interactive {
                self.interactive_owners.remove(&attachment.session_id);
            }
            attachment.state = SurfaceState::SessionClosed;
        }
        cancelled_requests
    }

    fn repoint_surface_to_session(&mut self, surface_id: &SurfaceId, new_session_id: SessionId) {
        let Some(old_session_id) = self.surface_to_session.get(surface_id).cloned() else {
            return;
        };
        if let Some(surfaces) = self.session_to_surfaces.get_mut(&old_session_id) {
            surfaces.remove(surface_id);
            if surfaces.is_empty() {
                self.session_to_surfaces.remove(&old_session_id);
            }
        }
        self.surface_to_session
            .insert(surface_id.clone(), new_session_id.clone());
        self.session_to_surfaces
            .entry(new_session_id.clone())
            .or_default()
            .insert(surface_id.clone());
        if let Some(attachment) = self.attachments.get_mut(surface_id) {
            if attachment.role == SurfaceRole::Interactive {
                self.interactive_owners.remove(&old_session_id);
                self.interactive_owners
                    .insert(new_session_id.clone(), surface_id.clone());
            }
            attachment.session_id = new_session_id;
            attachment.state = SurfaceState::Attached;
            attachment.last_delivered_seq = 0;
        }
    }

    fn insert_pending_server_request(&mut self, pending: PendingServerRequest) {
        self.pending_requests_by_session
            .entry(pending.session_id.clone())
            .or_default()
            .insert(pending.request_id.clone());
        self.pending_requests_by_surface
            .entry(pending.surface_id.clone())
            .or_default()
            .insert(pending.request_id.clone());
        if let Some(turn_id) = &pending.turn_id {
            self.pending_requests_by_turn
                .entry(turn_id.clone())
                .or_default()
                .insert(pending.request_id.clone());
        }
        self.pending_server_requests
            .insert(pending.request_id.clone(), pending);
    }

    fn remove_pending_server_request(
        &mut self,
        request_id: &RequestId,
    ) -> Option<PendingServerRequest> {
        let pending = self.pending_server_requests.remove(request_id)?;
        self.pending_server_request_payloads.remove(request_id);
        if let Some(requests) = self
            .pending_requests_by_session
            .get_mut(&pending.session_id)
        {
            requests.remove(request_id);
            if requests.is_empty() {
                self.pending_requests_by_session.remove(&pending.session_id);
            }
        }
        if let Some(requests) = self
            .pending_requests_by_surface
            .get_mut(&pending.surface_id)
        {
            requests.remove(request_id);
            if requests.is_empty() {
                self.pending_requests_by_surface.remove(&pending.surface_id);
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
            .filter(|request_id| self.remove_pending_server_request(request_id).is_some())
            .collect()
    }

    fn cancel_surface_server_requests(&mut self, surface_id: &SurfaceId) -> Vec<RequestId> {
        let Some(requests) = self.pending_requests_by_surface.remove(surface_id) else {
            return Vec::new();
        };
        requests
            .into_iter()
            .filter(|request_id| self.remove_pending_server_request(request_id).is_some())
            .collect()
    }

    fn validate_attach(
        &self,
        connection: ConnectionKey,
        surface_id: &SurfaceId,
        session_id: &SessionId,
        role: SurfaceRole,
    ) -> Result<(), AttachError> {
        let current_connection_count = self
            .connection_to_surfaces
            .get(&connection)
            .into_iter()
            .flat_map(|surfaces| surfaces.iter())
            .filter(|existing_surface| *existing_surface != surface_id)
            .filter(|existing_surface| {
                self.attachments
                    .get(*existing_surface)
                    .is_some_and(|attachment| attachment.state == SurfaceState::Attached)
            })
            .count();
        let same_connection_retarget = self
            .surface_to_connection
            .get(surface_id)
            .is_some_and(|current| current == &connection);
        if !same_connection_retarget
            && current_connection_count >= self.limits.max_surfaces_per_connection
        {
            return SurfaceLimitSnafu.fail();
        }

        match role {
            SurfaceRole::Interactive => {
                if let Some(owner_surface) = self.interactive_owners.get(session_id)
                    && owner_surface != surface_id
                {
                    // `interactive_owners` and `attachments` are kept in sync
                    // (crate invariant), so an interactive owner surface always
                    // has a live attachment entry here.
                    #[expect(
                        clippy::expect_used,
                        reason = "interactive_owners is kept in sync with attachments"
                    )]
                    let owner = self
                        .attachments
                        .get(owner_surface)
                        .expect("interactive owner must have attachment");
                    return InteractiveOwnerConflictSnafu {
                        session_id: session_id.clone(),
                        owner_surface: owner_surface.clone(),
                        owner_attached_at: owner.attached_at,
                        owner_idle: false,
                    }
                    .fail();
                }
            }
            SurfaceRole::Passive => {
                let passive_count = self
                    .session_to_surfaces
                    .get(session_id)
                    .into_iter()
                    .flat_map(|surfaces| surfaces.iter())
                    .filter(|existing_surface| *existing_surface != surface_id)
                    .filter(|existing_surface| {
                        self.attachments
                            .get(*existing_surface)
                            .is_some_and(|attachment| attachment.role == SurfaceRole::Passive)
                    })
                    .count();
                if passive_count >= self.limits.max_passive_surfaces_per_session {
                    return SurfaceLimitSnafu.fail();
                }
            }
        }

        Ok(())
    }
}

#[derive(Debug)]
struct RetentionRing {
    capacity: usize,
    envelopes: VecDeque<SessionEnvelope>,
    /// Highest durable seq this session has produced or been seeded with. It
    /// survives ring eviction, so an empty ring can still reject a stale
    /// cursor instead of accepting any cursor as "caught up".
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

    /// Seed the high-water baseline from a resume skip-ahead. A resumed
    /// session has emitted nothing yet in the new epoch, so its ring is empty;
    /// without this, a subscribe with an old cursor would attach live-only
    /// with no replay and silently miss the resume-to-first-event window.
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
            // Empty ring: strict. A cursor that is caught up to the
            // known high-water attaches live-only with nothing to replay; a
            // cursor behind it (e.g. `Some (0)` after a resume skip-ahead) fell
            // out of the ring and must re-baseline via a snapshot. A session
            // that never produced a durable event (`high_seq == None`) accepts
            // any cursor — there is nothing it could have missed.
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
