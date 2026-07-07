//! App-server session routing foundations.
//!
//! This crate is the first Phase A slice of `coco-app-server`: the internal
//! connection key, surface routing indexes, and per-session durable replay ring.
//! Runtime ownership, transports, and client adapters are wired in later slices.

mod app_server;
mod json_rpc_adapter;
mod local_client_adapter;
mod registry;

use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;

use chrono::DateTime;
use chrono::Utc;
use coco_error::ErrorExt;
use coco_error::Location;
use coco_error::StatusCode;
use coco_error::stack_trace_debug;
use coco_types::RequestId;
use coco_types::ServerRequest;
use coco_types::SessionEnvelope;
use coco_types::SessionId;
use coco_types::SurfaceId;
use coco_types::TurnId;
use snafu::Snafu;

pub use app_server::AppArchiveCommit;
pub use app_server::AppCloseStart;
pub use app_server::AppLiveSessionSummary;
pub use app_server::AppLoadStart;
pub use app_server::AppReplaceCommit;
pub use app_server::AppReplaceStart;
pub use app_server::AppServer;
pub use app_server::AppServerError;
pub use app_server::ResolvedServerRequest;
pub use app_server::ServerRequestReply;
pub use json_rpc_adapter::JsonRpcAdapter;
pub use json_rpc_adapter::JsonRpcAdapterConnection;
pub use json_rpc_adapter::JsonRpcAdapterError;
pub use json_rpc_adapter::JsonRpcServerRequestResponse;
pub use json_rpc_adapter::PendingJsonRpcServerRequest;
pub use local_client_adapter::LocalClientAdapter;
pub use local_client_adapter::LocalClientConnection;
pub use local_client_adapter::LocalClientSubscribeOutcome;
pub use local_client_adapter::LocalClientSubscription;
pub use local_client_adapter::LocalClientSurface;
pub use registry::CloseCompletion;
pub use registry::CloseStart;
pub use registry::LiveSessionRegistry;
pub use registry::LoadCompletion;
pub use registry::LoadStart;
pub use registry::RegistryError;
pub use registry::ReplaceCommit;
pub use registry::ReplaceStart;

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

/// One outbound delivery to a surface on a connection.
#[derive(Debug, Clone)]
pub struct SurfaceDelivery {
    pub surface_id: SurfaceId,
    pub envelope: SessionEnvelope,
}

pub type OutboundSender = tokio::sync::mpsc::Sender<SurfaceDelivery>;

#[derive(Debug, Clone)]
pub struct ServerRequestDelivery {
    pub surface_id: SurfaceId,
    pub request_id: RequestId,
    pub request: ServerRequest,
}

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
            SurfaceCapability::FilePicker => self.file_picker,
            SurfaceCapability::Keychain => self.keychain,
            SurfaceCapability::Attestation => self.attestation,
            SurfaceCapability::Notifications => self.notifications,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SurfaceCapability {
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
}

impl ErrorExt for AttachError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::InteractiveOwnerConflict { .. } => StatusCode::InvalidArguments,
            Self::SurfaceLimit { .. } => StatusCode::ResourcesExhausted,
            Self::SessionClosing { .. } => StatusCode::Cancelled,
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
pub struct ArchiveSessionOutcome {
    pub closed_surfaces: Vec<SurfaceId>,
    pub cancelled_requests: Vec<RequestId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceLifecycleEffect {
    pub surface_id: SurfaceId,
    pub kind: SurfaceLifecycleEffectKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceLifecycleEffectKind {
    SessionStarted {
        session_id: SessionId,
    },
    SessionReplaced {
        old_session_id: SessionId,
        new_session_id: SessionId,
    },
    SessionEnded {
        session_id: SessionId,
    },
}

#[derive(Debug, Clone)]
pub struct SurfaceLifecycleDelivery {
    pub surface_id: SurfaceId,
    pub effect: SurfaceLifecycleEffect,
}

pub type SurfaceLifecycleSender = tokio::sync::mpsc::Sender<SurfaceLifecycleDelivery>;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LifecycleRouteOutcome {
    pub delivered: usize,
    pub disconnected: Vec<ConnectionKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingServerRequest {
    pub request_id: RequestId,
    pub session_id: SessionId,
    pub surface_id: SurfaceId,
    pub capability: SurfaceCapability,
    pub turn_id: Option<TurnId>,
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
                surface_id,
                envelope: envelope.clone(),
            };
            match sender.try_send(delivery) {
                Ok(()) => outcome.delivered += 1,
                Err(tokio::sync::mpsc::error::TrySendError::Full(_))
                | Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    if !outcome.disconnected.contains(&connection) {
                        outcome.disconnected.push(connection);
                        self.disconnect(connection);
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
        for (connection, sender, effect) in deliveries {
            if outcome.disconnected.contains(&connection) {
                continue;
            }
            let delivery = SurfaceLifecycleDelivery {
                surface_id: effect.surface_id.clone(),
                effect,
            };
            match sender.try_send(delivery) {
                Ok(()) => outcome.delivered += 1,
                Err(tokio::sync::mpsc::error::TrySendError::Full(_))
                | Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    outcome.disconnected.push(connection);
                    self.disconnect(connection);
                }
            }
        }
        outcome
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
            surface_id: surface_id.clone(),
            request_id: pending.request_id.clone(),
            request,
        };
        match sender.try_send(delivery) {
            Ok(()) => Ok(ServerRequestRouteOutcome { pending }),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_))
            | Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                let request_id = pending.request_id;
                self.disconnect(connection);
                Err(ServerRequestRouteError::QueueUnavailable {
                    request_id,
                    surface_id,
                })
            }
        }
    }

    pub fn complete_server_request(
        &mut self,
        request_id: &RequestId,
        session_id: &SessionId,
    ) -> Result<PendingServerRequest, CompleteServerRequestError> {
        let Some(pending) = self.pending_server_requests.get(request_id) else {
            return Err(CompleteServerRequestError::NotFound {
                request_id: request_id.clone(),
            });
        };
        if &pending.session_id != session_id {
            return Err(CompleteServerRequestError::WrongSession {
                request_id: request_id.clone(),
                expected_session_id: pending.session_id.clone(),
                actual_session_id: session_id.clone(),
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
        replays.sort_by_key(|replay| replay.pending.request_id.as_display());
        replays
    }

    /// Re-point the calling surface to a replacement session and close peers.
    ///
    /// This is the `RoutingState` half of §7.5 Stage 2. Callers must invoke it
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
    pub fn archive_session(&mut self, session_id: &SessionId) -> ArchiveSessionOutcome {
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
        ArchiveSessionOutcome {
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
        PendingServerRequest {
            request_id: RequestId::String(format!(
                "server-request-{}",
                NEXT_SERVER_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
            )),
            session_id,
            surface_id,
            capability,
            turn_id,
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
}

impl RetentionRing {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            envelopes: VecDeque::new(),
        }
    }

    fn append(&mut self, envelope: SessionEnvelope) {
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
        let Some(oldest_seq) = self.envelopes.front().and_then(|env| env.session_seq) else {
            return RingReplay::Available(Vec::new());
        };
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
}

#[derive(Debug)]
enum RingReplay {
    Available(Vec<SessionEnvelope>),
    TooOld,
}

#[cfg(test)]
mod tests {
    use coco_types::CoreEvent;
    use coco_types::ServerNotification;
    use coco_types::ServerRequest;
    use coco_types::ServerRequestUserInputParams;
    use coco_types::SessionState;
    use coco_types::TuiOnlyEvent;

    use super::*;

    fn test_session_id(value: &str) -> SessionId {
        SessionId::try_new(value).expect("valid test session id")
    }

    fn durable_envelope(session_id: SessionId, seq: i64) -> SessionEnvelope {
        SessionEnvelope::durable(
            session_id,
            None,
            None,
            seq,
            CoreEvent::Protocol(ServerNotification::SessionStateChanged {
                state: SessionState::Running,
            }),
        )
    }

    fn ephemeral_envelope(session_id: SessionId) -> SessionEnvelope {
        SessionEnvelope::ephemeral(
            session_id,
            None,
            None,
            CoreEvent::Tui(TuiOnlyEvent::QuestionAsked {
                request_id: "question-1".to_string(),
                input: serde_json::json!({ "question": "continue?" }),
            }),
        )
    }

    fn request_id_strings(request_ids: Vec<RequestId>) -> Vec<String> {
        let mut request_ids = request_ids
            .into_iter()
            .map(|request_id| request_id.as_display())
            .collect::<Vec<_>>();
        request_ids.sort();
        request_ids
    }

    fn test_server_request() -> ServerRequest {
        ServerRequest::RequestUserInput(ServerRequestUserInputParams {
            request_id: "payload-request-id".to_string(),
            prompt: "continue?".to_string(),
            description: None,
            choices: Vec::new(),
            default: None,
        })
    }

    #[test]
    fn subscribe_replays_ring_then_receives_live_events() {
        let mut routing = RoutingState::new(8);
        let session_id = test_session_id("sess-1");
        routing.route_envelope(durable_envelope(session_id.clone(), 1));
        routing.route_envelope(durable_envelope(session_id.clone(), 2));
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let connection = ConnectionKey::for_test(1);
        let surface_id = SurfaceId::from("surface-1");
        routing.connect(connection, tx);

        let replay = routing
            .subscribe(connection, surface_id.clone(), session_id.clone(), Some(1))
            .expect("subscribe");

        let SubscribeReplay::Replayed(events) = replay else {
            panic!("expected replay");
        };
        assert_eq!(
            events
                .iter()
                .map(|event| event.session_seq.expect("seq"))
                .collect::<Vec<_>>(),
            vec![2]
        );
        assert_eq!(routing.surface_session(&surface_id), Some(&session_id));

        let outcome = routing.route_envelope(durable_envelope(session_id, 3));
        assert_eq!(outcome.delivered, 1);
        let delivered = rx.try_recv().expect("live delivery");
        assert_eq!(delivered.surface_id, surface_id);
        assert_eq!(delivered.envelope.session_seq, Some(3));
    }

    #[test]
    fn subscribe_requires_snapshot_when_cursor_falls_out_of_ring() {
        let mut routing = RoutingState::new(2);
        let session_id = test_session_id("sess-1");
        routing.route_envelope(durable_envelope(session_id.clone(), 1));
        routing.route_envelope(durable_envelope(session_id.clone(), 2));
        routing.route_envelope(durable_envelope(session_id.clone(), 3));
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let connection = ConnectionKey::for_test(1);
        let surface_id = SurfaceId::from("surface-1");
        routing.connect(connection, tx);

        let replay = routing
            .subscribe(connection, surface_id.clone(), session_id.clone(), Some(0))
            .expect("subscribe");

        assert!(matches!(replay, SubscribeReplay::SnapshotRequired));
        assert_eq!(routing.surface_session(&surface_id), None);
        let outcome = routing.route_envelope(durable_envelope(session_id, 4));
        assert_eq!(outcome.delivered, 0);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn missing_cursor_requires_snapshot_and_does_not_attach() {
        let mut routing = RoutingState::new(8);
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let connection = ConnectionKey::for_test(1);
        let surface_id = SurfaceId::from("surface-1");
        let session_id = test_session_id("sess-1");
        routing.connect(connection, tx);

        let replay = routing
            .subscribe(connection, surface_id.clone(), session_id, None)
            .expect("subscribe");

        assert!(matches!(replay, SubscribeReplay::SnapshotRequired));
        assert_eq!(routing.surface_session(&surface_id), None);
    }

    #[test]
    fn ephemeral_events_deliver_live_without_entering_replay_ring() {
        let mut routing = RoutingState::new(8);
        let session_id = test_session_id("sess-1");
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let connection = ConnectionKey::for_test(1);
        let surface_id = SurfaceId::from("surface-1");
        routing.connect(connection, tx);
        routing
            .attach_surface(connection, surface_id.clone(), session_id.clone())
            .expect("attach");

        let outcome = routing.route_envelope(ephemeral_envelope(session_id.clone()));

        assert_eq!(outcome.delivered, 1);
        let delivered = rx.try_recv().expect("ephemeral delivery");
        assert_eq!(delivered.surface_id, surface_id);
        assert_eq!(delivered.envelope.session_seq, None);

        let replay = routing
            .subscribe(
                connection,
                SurfaceId::from("surface-2"),
                session_id,
                Some(0),
            )
            .expect("subscribe");
        let SubscribeReplay::Replayed(events) = replay else {
            panic!("expected empty replay");
        };
        assert!(events.is_empty());
    }

    #[test]
    fn disconnect_removes_surfaces_from_all_indexes() {
        let mut routing = RoutingState::new(8);
        let session_id = test_session_id("sess-1");
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let connection = ConnectionKey::for_test(1);
        let surface_1 = SurfaceId::from("surface-1");
        let surface_2 = SurfaceId::from("surface-2");
        routing.connect(connection, tx);
        routing
            .attach_surface(connection, surface_1.clone(), session_id.clone())
            .expect("attach surface 1");
        routing
            .attach_surface(connection, surface_2.clone(), session_id.clone())
            .expect("attach surface 2");

        let outcome = routing.disconnect(connection);

        assert_eq!(outcome.detached_surfaces.len(), 2);
        assert!(outcome.cancelled_requests.is_empty());
        assert_eq!(routing.surface_session(&surface_1), None);
        assert_eq!(routing.surface_session(&surface_2), None);
        assert_eq!(routing.connection_surface_count(connection), 0);
        let outcome = routing.route_envelope(durable_envelope(session_id, 1));
        assert_eq!(outcome.delivered, 0);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn slow_consumer_disconnects_connection() {
        let mut routing = RoutingState::new(8);
        let session_id = test_session_id("sess-1");
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let connection = ConnectionKey::for_test(1);
        let surface_id = SurfaceId::from("surface-1");
        routing.connect(connection, tx);
        routing
            .attach_surface(connection, surface_id.clone(), session_id.clone())
            .expect("attach");

        let first = routing.route_envelope(durable_envelope(session_id.clone(), 1));
        let second = routing.route_envelope(durable_envelope(session_id.clone(), 2));

        assert_eq!(first.delivered, 1);
        assert_eq!(second.delivered, 0);
        assert_eq!(second.disconnected, vec![connection]);
        assert_eq!(routing.surface_session(&surface_id), None);
        assert_eq!(routing.connection_surface_count(connection), 0);

        let queued = rx.try_recv().expect("first delivery remains queued");
        assert_eq!(queued.envelope.session_seq, Some(1));
        let third = routing.route_envelope(durable_envelope(session_id, 3));
        assert_eq!(third.delivered, 0);
    }

    #[test]
    fn second_interactive_surface_is_rejected_with_owner_metadata() {
        let mut routing = RoutingState::new(8);
        let session_id = test_session_id("sess-1");
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let connection = ConnectionKey::for_test(1);
        let owner_surface = SurfaceId::from("surface-owner");
        let second_surface = SurfaceId::from("surface-second");
        routing.connect(connection, tx);
        let options = AttachSurfaceOptions {
            role: SurfaceRole::Interactive,
            ..AttachSurfaceOptions::default()
        };
        routing
            .attach_surface_with_options(
                connection,
                owner_surface.clone(),
                session_id.clone(),
                options.clone(),
            )
            .expect("attach interactive owner");

        let err = routing
            .attach_surface_with_options(connection, second_surface, session_id.clone(), options)
            .expect_err("second interactive should be rejected");

        match err {
            AttachError::InteractiveOwnerConflict {
                session_id: err_session_id,
                owner_surface: err_owner_surface,
                owner_idle,
                ..
            } => {
                assert_eq!(err_session_id, session_id);
                assert_eq!(err_owner_surface, owner_surface);
                assert!(!owner_idle);
            }
            other => panic!("unexpected error: {other:?}"),
        }
        assert_eq!(routing.interactive_owner(&session_id), Some(&owner_surface));
    }

    #[test]
    fn passive_surfaces_can_share_session_with_interactive_owner() {
        let mut routing = RoutingState::new(8);
        let session_id = test_session_id("sess-1");
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let connection = ConnectionKey::for_test(1);
        let interactive = SurfaceId::from("surface-interactive");
        let passive = SurfaceId::from("surface-passive");
        routing.connect(connection, tx);
        routing
            .attach_surface_with_options(
                connection,
                interactive.clone(),
                session_id.clone(),
                AttachSurfaceOptions {
                    role: SurfaceRole::Interactive,
                    ..AttachSurfaceOptions::default()
                },
            )
            .expect("attach interactive");
        routing
            .attach_surface(connection, passive.clone(), session_id.clone())
            .expect("attach passive");

        assert_eq!(routing.interactive_owner(&session_id), Some(&interactive));
        assert_eq!(routing.surface_session(&passive), Some(&session_id));
    }

    #[test]
    fn connection_surface_limit_is_enforced() {
        let mut routing = RoutingState::new_with_limits(
            8,
            SurfaceLimits {
                max_surfaces_per_connection: 1,
                max_passive_surfaces_per_session: 16,
            },
        );
        let session_id = test_session_id("sess-1");
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let connection = ConnectionKey::for_test(1);
        routing.connect(connection, tx);
        routing
            .attach_surface(connection, SurfaceId::from("surface-1"), session_id.clone())
            .expect("attach first");

        let err = routing
            .attach_surface(connection, SurfaceId::from("surface-2"), session_id)
            .expect_err("second surface should exceed connection limit");

        assert!(matches!(err, AttachError::SurfaceLimit { .. }));
        assert_eq!(err.status_code(), StatusCode::ResourcesExhausted);
    }

    #[test]
    fn passive_surface_limit_is_enforced_per_session() {
        let mut routing = RoutingState::new_with_limits(
            8,
            SurfaceLimits {
                max_surfaces_per_connection: 8,
                max_passive_surfaces_per_session: 1,
            },
        );
        let session_id = test_session_id("sess-1");
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let connection = ConnectionKey::for_test(1);
        routing.connect(connection, tx);
        routing
            .attach_surface(connection, SurfaceId::from("surface-1"), session_id.clone())
            .expect("attach first passive");

        let err = routing
            .attach_surface(connection, SurfaceId::from("surface-2"), session_id)
            .expect_err("second passive should exceed session passive limit");

        assert!(matches!(err, AttachError::SurfaceLimit { .. }));
    }

    #[test]
    fn notification_preferences_filter_delivery_per_surface() {
        let mut routing = RoutingState::new(8);
        let session_id = test_session_id("sess-1");
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let connection = ConnectionKey::for_test(1);
        let surface_id = SurfaceId::from("surface-1");
        routing.connect(connection, tx);
        routing
            .attach_surface_with_options(
                connection,
                surface_id,
                session_id.clone(),
                AttachSurfaceOptions {
                    notification_prefs: NotificationPrefs {
                        protocol: true,
                        stream: true,
                        tui: false,
                    },
                    ..AttachSurfaceOptions::default()
                },
            )
            .expect("attach");

        let tui_outcome = routing.route_envelope(ephemeral_envelope(session_id.clone()));
        let protocol_outcome = routing.route_envelope(durable_envelope(session_id, 1));

        assert_eq!(tui_outcome.delivered, 0);
        assert_eq!(protocol_outcome.delivered, 1);
        let delivered = rx.try_recv().expect("protocol delivery");
        assert_eq!(delivered.envelope.session_seq, Some(1));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn disconnect_clears_interactive_owner() {
        let mut routing = RoutingState::new(8);
        let session_id = test_session_id("sess-1");
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let connection = ConnectionKey::for_test(1);
        let surface_id = SurfaceId::from("surface-1");
        routing.connect(connection, tx);
        routing
            .attach_surface_with_options(
                connection,
                surface_id.clone(),
                session_id.clone(),
                AttachSurfaceOptions {
                    role: SurfaceRole::Interactive,
                    ..AttachSurfaceOptions::default()
                },
            )
            .expect("attach interactive");

        routing.disconnect(connection);

        assert_eq!(routing.interactive_owner(&session_id), None);
        assert_eq!(
            routing.surface_attachment(&surface_id).map(|a| a.state),
            None
        );
    }

    #[test]
    fn server_request_targets_interactive_surface_with_declared_capability() {
        let mut routing = RoutingState::new(8);
        let session_id = test_session_id("sess-1");
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let connection = ConnectionKey::for_test(1);
        let interactive = SurfaceId::from("surface-interactive");
        let passive = SurfaceId::from("surface-passive");
        routing.connect(connection, tx);
        routing
            .attach_surface(connection, passive.clone(), session_id.clone())
            .expect("attach passive");
        routing
            .attach_surface_with_options(
                connection,
                interactive.clone(),
                session_id.clone(),
                AttachSurfaceOptions {
                    role: SurfaceRole::Interactive,
                    capabilities: SurfaceCapabilities {
                        keychain: true,
                        ..SurfaceCapabilities::default()
                    },
                    ..AttachSurfaceOptions::default()
                },
            )
            .expect("attach interactive");

        let pending = routing
            .open_server_request(session_id.clone(), SurfaceCapability::Keychain, None)
            .expect("open request");

        assert_eq!(pending.session_id, session_id);
        assert_eq!(pending.surface_id, interactive);
        assert_eq!(pending.capability, SurfaceCapability::Keychain);
        assert!(
            routing
                .pending_server_requests_for_surface(&passive)
                .is_empty()
        );
        assert_eq!(
            routing.pending_server_requests_for_surface(&pending.surface_id),
            vec![pending]
        );
    }

    #[test]
    fn route_server_request_delivers_on_request_channel_and_records_pending() {
        let mut routing = RoutingState::new(8);
        let session_id = test_session_id("sess-1");
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(8);
        let (request_tx, mut request_rx) = tokio::sync::mpsc::channel(8);
        let connection = ConnectionKey::for_test(1);
        let surface_id = SurfaceId::from("surface-1");
        routing.connect_with_request_sender(connection, event_tx, request_tx);
        routing
            .attach_surface_with_options(
                connection,
                surface_id.clone(),
                session_id.clone(),
                AttachSurfaceOptions {
                    role: SurfaceRole::Interactive,
                    capabilities: SurfaceCapabilities {
                        notifications: true,
                        ..SurfaceCapabilities::default()
                    },
                    ..AttachSurfaceOptions::default()
                },
            )
            .expect("attach interactive");

        let outcome = routing
            .route_server_request(
                session_id.clone(),
                SurfaceCapability::Notifications,
                Some(TurnId::from("turn-1")),
                test_server_request(),
            )
            .expect("route request");

        assert_eq!(outcome.pending.session_id, session_id);
        assert_eq!(outcome.pending.surface_id, surface_id);
        assert_eq!(
            routing.pending_server_requests_for_surface(&outcome.pending.surface_id),
            vec![outcome.pending.clone()]
        );
        let delivery = request_rx.try_recv().expect("request delivery");
        assert_eq!(delivery.surface_id, outcome.pending.surface_id);
        assert_eq!(delivery.request_id, outcome.pending.request_id);
        assert!(matches!(
            delivery.request,
            ServerRequest::RequestUserInput(_)
        ));
        assert!(event_rx.try_recv().is_err());
    }

    #[test]
    fn route_server_request_replay_returns_retained_payload_for_surface() {
        let mut routing = RoutingState::new(8);
        let session_id = test_session_id("sess-1");
        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
        let (request_tx, _request_rx) = tokio::sync::mpsc::channel(8);
        let connection = ConnectionKey::for_test(1);
        let surface_id = SurfaceId::from("surface-1");
        routing.connect_with_request_sender(connection, event_tx, request_tx);
        routing
            .attach_surface_with_options(
                connection,
                surface_id.clone(),
                session_id.clone(),
                AttachSurfaceOptions {
                    role: SurfaceRole::Interactive,
                    capabilities: SurfaceCapabilities {
                        notifications: true,
                        ..SurfaceCapabilities::default()
                    },
                    ..AttachSurfaceOptions::default()
                },
            )
            .expect("attach interactive");

        let outcome = routing
            .route_server_request(
                session_id,
                SurfaceCapability::Notifications,
                Some(TurnId::from("turn-1")),
                test_server_request(),
            )
            .expect("route request");

        let replays = routing.pending_server_request_replays_for_surface(&surface_id);
        assert_eq!(replays.len(), 1);
        assert_eq!(replays[0].pending, outcome.pending);
        let ServerRequest::RequestUserInput(params) = &replays[0].request else {
            panic!("expected user input replay");
        };
        assert_eq!(params.request_id, "payload-request-id");
        assert_eq!(params.prompt, "continue?");
    }

    #[test]
    fn completed_routed_server_request_removes_replay_payload() {
        let mut routing = RoutingState::new(8);
        let session_id = test_session_id("sess-1");
        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
        let (request_tx, _request_rx) = tokio::sync::mpsc::channel(8);
        let connection = ConnectionKey::for_test(1);
        let surface_id = SurfaceId::from("surface-1");
        routing.connect_with_request_sender(connection, event_tx, request_tx);
        routing
            .attach_surface_with_options(
                connection,
                surface_id.clone(),
                session_id.clone(),
                AttachSurfaceOptions {
                    role: SurfaceRole::Interactive,
                    capabilities: SurfaceCapabilities {
                        notifications: true,
                        ..SurfaceCapabilities::default()
                    },
                    ..AttachSurfaceOptions::default()
                },
            )
            .expect("attach interactive");
        let outcome = routing
            .route_server_request(
                session_id.clone(),
                SurfaceCapability::Notifications,
                None,
                test_server_request(),
            )
            .expect("route request");

        routing
            .complete_server_request(&outcome.pending.request_id, &session_id)
            .expect("complete request");

        assert!(
            routing
                .pending_server_request_replays_for_surface(&surface_id)
                .is_empty()
        );
    }

    #[test]
    fn route_server_request_requires_request_channel_without_opening_pending() {
        let mut routing = RoutingState::new(8);
        let session_id = test_session_id("sess-1");
        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
        let connection = ConnectionKey::for_test(1);
        let surface_id = SurfaceId::from("surface-1");
        routing.connect(connection, event_tx);
        routing
            .attach_surface_with_options(
                connection,
                surface_id.clone(),
                session_id.clone(),
                AttachSurfaceOptions {
                    role: SurfaceRole::Interactive,
                    capabilities: SurfaceCapabilities {
                        keychain: true,
                        ..SurfaceCapabilities::default()
                    },
                    ..AttachSurfaceOptions::default()
                },
            )
            .expect("attach interactive");

        let err = routing
            .route_server_request(
                session_id,
                SurfaceCapability::Keychain,
                None,
                test_server_request(),
            )
            .expect_err("missing request sender");

        assert_eq!(
            err,
            ServerRequestRouteError::NoRequestChannel {
                surface_id: surface_id.clone(),
            }
        );
        assert!(
            routing
                .pending_server_requests_for_surface(&surface_id)
                .is_empty()
        );
    }

    #[test]
    fn route_server_request_disconnects_full_request_channel_and_cancels_pending() {
        let mut routing = RoutingState::new(8);
        let session_id = test_session_id("sess-1");
        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
        let (request_tx, mut request_rx) = tokio::sync::mpsc::channel(1);
        let connection = ConnectionKey::for_test(1);
        let surface_id = SurfaceId::from("surface-1");
        routing.connect_with_request_sender(connection, event_tx, request_tx);
        routing
            .attach_surface_with_options(
                connection,
                surface_id.clone(),
                session_id.clone(),
                AttachSurfaceOptions {
                    role: SurfaceRole::Interactive,
                    capabilities: SurfaceCapabilities {
                        notifications: true,
                        ..SurfaceCapabilities::default()
                    },
                    ..AttachSurfaceOptions::default()
                },
            )
            .expect("attach interactive");

        let first = routing
            .route_server_request(
                session_id.clone(),
                SurfaceCapability::Notifications,
                None,
                test_server_request(),
            )
            .expect("first route");
        let second = routing
            .route_server_request(
                session_id.clone(),
                SurfaceCapability::Notifications,
                None,
                test_server_request(),
            )
            .expect_err("request channel full");

        let ServerRequestRouteError::QueueUnavailable { request_id, .. } = second else {
            panic!("expected queue unavailable");
        };
        assert_ne!(request_id, first.pending.request_id);
        assert_eq!(routing.surface_session(&surface_id), None);
        assert!(
            routing
                .pending_server_requests_for_surface(&surface_id)
                .is_empty()
        );
        assert!(matches!(
            routing.complete_server_request(&first.pending.request_id, &session_id),
            Err(CompleteServerRequestError::NotFound { .. })
        ));
        let queued = request_rx.try_recv().expect("first request remains queued");
        assert_eq!(queued.request_id, first.pending.request_id);
    }

    #[test]
    fn server_request_rejects_missing_interactive_capability() {
        let mut routing = RoutingState::new(8);
        let session_id = test_session_id("sess-1");
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let connection = ConnectionKey::for_test(1);
        let interactive = SurfaceId::from("surface-interactive");
        routing.connect(connection, tx);
        routing
            .attach_surface_with_options(
                connection,
                interactive.clone(),
                session_id.clone(),
                AttachSurfaceOptions {
                    role: SurfaceRole::Interactive,
                    capabilities: SurfaceCapabilities {
                        keychain: true,
                        ..SurfaceCapabilities::default()
                    },
                    ..AttachSurfaceOptions::default()
                },
            )
            .expect("attach interactive");

        let err = routing
            .open_server_request(session_id.clone(), SurfaceCapability::FilePicker, None)
            .expect_err("file picker was not declared");

        assert_eq!(
            err,
            OpenServerRequestError::CapabilityNotDeclared {
                session_id,
                surface_id: interactive,
                capability: SurfaceCapability::FilePicker,
            }
        );
    }

    #[test]
    fn completing_server_request_validates_session_and_clears_indexes() {
        let mut routing = RoutingState::new(8);
        let session_id = test_session_id("sess-1");
        let wrong_session_id = test_session_id("sess-wrong");
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let connection = ConnectionKey::for_test(1);
        let surface_id = SurfaceId::from("surface-1");
        routing.connect(connection, tx);
        routing
            .attach_surface_with_options(
                connection,
                surface_id.clone(),
                session_id.clone(),
                AttachSurfaceOptions {
                    role: SurfaceRole::Interactive,
                    capabilities: SurfaceCapabilities {
                        notifications: true,
                        ..SurfaceCapabilities::default()
                    },
                    ..AttachSurfaceOptions::default()
                },
            )
            .expect("attach interactive");
        let pending = routing
            .open_server_request(
                session_id.clone(),
                SurfaceCapability::Notifications,
                Some(TurnId::from("turn-1")),
            )
            .expect("open request");

        let err = routing
            .complete_server_request(&pending.request_id, &wrong_session_id)
            .expect_err("wrong session should be rejected");
        assert_eq!(
            err,
            CompleteServerRequestError::WrongSession {
                request_id: pending.request_id.clone(),
                expected_session_id: session_id.clone(),
                actual_session_id: wrong_session_id,
            }
        );

        let completed = routing
            .complete_server_request(&pending.request_id, &session_id)
            .expect("complete request");
        assert_eq!(completed, pending);
        assert!(
            routing
                .pending_server_requests_for_surface(&surface_id)
                .is_empty()
        );
        assert!(matches!(
            routing.complete_server_request(&completed.request_id, &session_id),
            Err(CompleteServerRequestError::NotFound { .. })
        ));
    }

    #[test]
    fn disconnect_cancels_pending_requests_for_connection_surfaces() {
        let mut routing = RoutingState::new(8);
        let session_id = test_session_id("sess-1");
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let connection = ConnectionKey::for_test(1);
        let surface_id = SurfaceId::from("surface-1");
        routing.connect(connection, tx);
        routing
            .attach_surface_with_options(
                connection,
                surface_id.clone(),
                session_id.clone(),
                AttachSurfaceOptions {
                    role: SurfaceRole::Interactive,
                    capabilities: SurfaceCapabilities {
                        keychain: true,
                        notifications: true,
                        ..SurfaceCapabilities::default()
                    },
                    ..AttachSurfaceOptions::default()
                },
            )
            .expect("attach interactive");
        let keychain = routing
            .open_server_request(session_id.clone(), SurfaceCapability::Keychain, None)
            .expect("open keychain");
        let notifications = routing
            .open_server_request(session_id.clone(), SurfaceCapability::Notifications, None)
            .expect("open notifications");

        let outcome = routing.disconnect(connection);

        assert_eq!(outcome.detached_surfaces, vec![surface_id.clone()]);
        assert_eq!(
            request_id_strings(outcome.cancelled_requests),
            request_id_strings(vec![keychain.request_id.clone(), notifications.request_id])
        );
        assert!(
            routing
                .pending_server_requests_for_surface(&surface_id)
                .is_empty()
        );
        assert!(matches!(
            routing.complete_server_request(&keychain.request_id, &session_id),
            Err(CompleteServerRequestError::NotFound { .. })
        ));
    }

    #[test]
    fn turn_transition_cancels_only_that_turns_pending_requests() {
        let mut routing = RoutingState::new(8);
        let session_id = test_session_id("sess-1");
        let turn_1 = TurnId::from("turn-1");
        let turn_2 = TurnId::from("turn-2");
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let connection = ConnectionKey::for_test(1);
        let surface_id = SurfaceId::from("surface-1");
        routing.connect(connection, tx);
        routing
            .attach_surface_with_options(
                connection,
                surface_id.clone(),
                session_id.clone(),
                AttachSurfaceOptions {
                    role: SurfaceRole::Interactive,
                    capabilities: SurfaceCapabilities {
                        notifications: true,
                        ..SurfaceCapabilities::default()
                    },
                    ..AttachSurfaceOptions::default()
                },
            )
            .expect("attach interactive");
        let first = routing
            .open_server_request(
                session_id.clone(),
                SurfaceCapability::Notifications,
                Some(turn_1.clone()),
            )
            .expect("open first");
        let second = routing
            .open_server_request(
                session_id.clone(),
                SurfaceCapability::Notifications,
                Some(turn_2),
            )
            .expect("open second");

        let cancelled = routing.cancel_turn_server_requests(&turn_1);

        assert_eq!(cancelled, vec![first.request_id.clone()]);
        assert_eq!(
            routing.pending_server_requests_for_surface(&surface_id),
            vec![second.clone()]
        );
        assert!(matches!(
            routing.complete_server_request(&first.request_id, &session_id),
            Err(CompleteServerRequestError::NotFound { .. })
        ));
        assert_eq!(
            routing
                .complete_server_request(&second.request_id, &session_id)
                .expect("second still pending"),
            second
        );
    }

    #[test]
    fn replace_repoints_calling_surface_and_closes_peer_surfaces() {
        let mut routing = RoutingState::new(8);
        let old_session_id = test_session_id("sess-old");
        let new_session_id = test_session_id("sess-new");
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let connection = ConnectionKey::for_test(1);
        let caller = SurfaceId::from("surface-caller");
        let peer = SurfaceId::from("surface-peer");
        routing.connect(connection, tx);
        routing
            .attach_surface_with_options(
                connection,
                caller.clone(),
                old_session_id.clone(),
                AttachSurfaceOptions {
                    role: SurfaceRole::Interactive,
                    last_delivered_seq: 42,
                    ..AttachSurfaceOptions::default()
                },
            )
            .expect("attach caller");
        routing
            .attach_surface(connection, peer.clone(), old_session_id.clone())
            .expect("attach peer");

        let outcome = routing
            .replace_calling_surface(&caller, new_session_id.clone())
            .expect("replace");

        assert_eq!(outcome.old_session_id, old_session_id);
        assert_eq!(outcome.new_session_id, new_session_id);
        assert_eq!(outcome.calling_surface, caller);
        assert_eq!(outcome.detached_surfaces, vec![peer.clone()]);
        assert!(outcome.cancelled_requests.is_empty());
        assert_eq!(routing.surface_session(&caller), Some(&new_session_id));
        assert_eq!(routing.surface_session(&peer), None);
        assert_eq!(
            routing.surface_attachment(&peer).map(|a| a.state),
            Some(SurfaceState::SessionClosed)
        );
        assert_eq!(routing.interactive_owner(&old_session_id), None);
        assert_eq!(routing.interactive_owner(&new_session_id), Some(&caller));
        assert_eq!(
            routing
                .surface_attachment(&caller)
                .map(|a| a.last_delivered_seq),
            Some(0)
        );

        let old_outcome = routing.route_envelope(durable_envelope(old_session_id, 1));
        let new_outcome = routing.route_envelope(durable_envelope(new_session_id, 1));
        assert_eq!(old_outcome.delivered, 0);
        assert_eq!(new_outcome.delivered, 1);
        let delivered = rx.try_recv().expect("new-session delivery");
        assert_eq!(delivered.surface_id, caller);
        assert_eq!(delivered.envelope.session_id, test_session_id("sess-new"));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn replace_cancels_old_session_pending_requests() {
        let mut routing = RoutingState::new(8);
        let old_session_id = test_session_id("sess-old");
        let new_session_id = test_session_id("sess-new");
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let connection = ConnectionKey::for_test(1);
        let caller = SurfaceId::from("surface-caller");
        routing.connect(connection, tx);
        routing
            .attach_surface_with_options(
                connection,
                caller.clone(),
                old_session_id.clone(),
                AttachSurfaceOptions {
                    role: SurfaceRole::Interactive,
                    capabilities: SurfaceCapabilities {
                        keychain: true,
                        ..SurfaceCapabilities::default()
                    },
                    ..AttachSurfaceOptions::default()
                },
            )
            .expect("attach caller");
        let pending = routing
            .open_server_request(old_session_id.clone(), SurfaceCapability::Keychain, None)
            .expect("open request");

        let outcome = routing
            .replace_calling_surface(&caller, new_session_id.clone())
            .expect("replace");

        assert_eq!(outcome.cancelled_requests, vec![pending.request_id.clone()]);
        assert_eq!(routing.surface_session(&caller), Some(&new_session_id));
        assert!(
            routing
                .pending_server_requests_for_surface(&caller)
                .is_empty()
        );
        assert!(matches!(
            routing.complete_server_request(&pending.request_id, &old_session_id),
            Err(CompleteServerRequestError::NotFound { .. })
        ));
    }

    #[test]
    fn archive_session_closes_surfaces_and_removes_fanout() {
        let mut routing = RoutingState::new(8);
        let session_id = test_session_id("sess-1");
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let connection = ConnectionKey::for_test(1);
        let interactive = SurfaceId::from("surface-interactive");
        let passive = SurfaceId::from("surface-passive");
        routing.connect(connection, tx);
        routing
            .attach_surface_with_options(
                connection,
                interactive.clone(),
                session_id.clone(),
                AttachSurfaceOptions {
                    role: SurfaceRole::Interactive,
                    ..AttachSurfaceOptions::default()
                },
            )
            .expect("attach interactive");
        routing
            .attach_surface(connection, passive.clone(), session_id.clone())
            .expect("attach passive");

        let outcome = routing.archive_session(&session_id);

        assert_eq!(outcome.closed_surfaces.len(), 2);
        assert!(outcome.cancelled_requests.is_empty());
        assert_eq!(routing.surface_session(&interactive), None);
        assert_eq!(routing.surface_session(&passive), None);
        assert_eq!(
            routing.surface_attachment(&interactive).map(|a| a.state),
            Some(SurfaceState::SessionClosed)
        );
        assert_eq!(
            routing.surface_attachment(&passive).map(|a| a.state),
            Some(SurfaceState::SessionClosed)
        );
        assert_eq!(routing.interactive_owner(&session_id), None);
        assert_eq!(routing.attached_connection_surface_count(connection), 0);
        assert_eq!(routing.connection_surface_count(connection), 2);

        let route = routing.route_envelope(durable_envelope(session_id, 1));
        assert_eq!(route.delivered, 0);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn archive_session_cancels_pending_requests() {
        let mut routing = RoutingState::new(8);
        let session_id = test_session_id("sess-1");
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let connection = ConnectionKey::for_test(1);
        let surface_id = SurfaceId::from("surface-1");
        routing.connect(connection, tx);
        routing
            .attach_surface_with_options(
                connection,
                surface_id.clone(),
                session_id.clone(),
                AttachSurfaceOptions {
                    role: SurfaceRole::Interactive,
                    capabilities: SurfaceCapabilities {
                        notifications: true,
                        ..SurfaceCapabilities::default()
                    },
                    ..AttachSurfaceOptions::default()
                },
            )
            .expect("attach interactive");
        let pending = routing
            .open_server_request(session_id.clone(), SurfaceCapability::Notifications, None)
            .expect("open request");

        let outcome = routing.archive_session(&session_id);

        assert_eq!(outcome.cancelled_requests, vec![pending.request_id.clone()]);
        assert!(
            routing
                .pending_server_requests_for_surface(&surface_id)
                .is_empty()
        );
        assert!(matches!(
            routing.complete_server_request(&pending.request_id, &session_id),
            Err(CompleteServerRequestError::NotFound { .. })
        ));
    }

    #[test]
    fn closed_surfaces_do_not_count_against_connection_limit() {
        let mut routing = RoutingState::new_with_limits(
            8,
            SurfaceLimits {
                max_surfaces_per_connection: 1,
                max_passive_surfaces_per_session: 16,
            },
        );
        let first_session_id = test_session_id("sess-1");
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let connection = ConnectionKey::for_test(1);
        let first = SurfaceId::from("surface-1");
        let second = SurfaceId::from("surface-2");
        routing.connect(connection, tx);
        routing
            .attach_surface(connection, first.clone(), first_session_id.clone())
            .expect("attach first");
        routing.archive_session(&first_session_id);

        routing
            .attach_surface(connection, second.clone(), test_session_id("sess-2"))
            .expect("closed first surface should not consume live limit");

        assert_eq!(routing.connection_surface_count(connection), 2);
        assert_eq!(routing.attached_connection_surface_count(connection), 1);
        assert_eq!(
            routing.surface_attachment(&first).map(|a| a.state),
            Some(SurfaceState::SessionClosed)
        );
        assert_eq!(
            routing.surface_session(&second),
            Some(&test_session_id("sess-2"))
        );
    }
}
