use std::{
    future::Future,
    sync::{Arc, RwLock},
    time::Instant,
};

use coco_error::{ErrorExt, Location, StatusCode, stack_trace_debug};
use coco_types::{
    ApprovalResolveParams, ElicitationResolveParams, InteractiveTarget, RequestId, ServerRequest,
    SessionEnvelope, SessionId, SurfaceId, SurfaceLifecycleEffect, SurfaceLifecycleEffectKind,
    TurnId, UserInputResolveParams,
};
use snafu::{IntoError, ResultExt, Snafu};

use crate::{
    AttachError, AttachSurfaceOptions, CloseCompletion, CloseSessionSurfacesOutcome, CloseStart,
    CompleteServerRequestError, ConnectionKey, DetachSurfaceOutcome, DisconnectOutcome,
    LifecycleRouteOutcome, LiveSessionRegistry, LoadCompletion, LoadStart, OutboundSender,
    PendingServerRequest, PendingServerRequestReplay, ReplaceStart, ReplaceSurfaceOutcome,
    RouteOutcome, RoutingState, ServerRequestRouteError, ServerRequestRouteOutcome,
    ServerRequestSender, SessionActivityTracker, SessionSurfaceCounts, SubscribeReplay,
    SurfaceAttachment, SurfaceCapability, SurfaceLifecycleSender, SurfaceLimits, SurfaceRole,
    SurfaceState,
    registry::{CloseState, ClosingState, RegistryError, SessionSlot},
};

/// App-server state holder for registry + routing lock ordering.
///
/// It owns lifecycle owner tasks and the no-await commit sections that touch
/// both lifecycle slots and surface routing. Runtime construction and close
/// behavior remain opaque futures supplied by the application host.
pub struct AppServer<H> {
    registry: LiveSessionRegistry<H>,
    routing: RwLock<RoutingState>,
    activity: SessionActivityTracker,
    server_request_waiters: std::sync::Mutex<
        std::collections::HashMap<RequestId, tokio::sync::oneshot::Sender<ServerRequestReply>>,
    >,
    /// Retained join handles for lifecycle owner tasks (load/close/replace) so
    /// process shutdown can abort and join any still in flight rather than
    /// detaching them (CS-3c).
    owner_tasks: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl<H: Clone> AppServer<H> {
    pub fn new(max_sessions: usize, retention_per_session: usize) -> Self {
        Self::new_with_surface_limits(
            max_sessions,
            retention_per_session,
            SurfaceLimits::default(),
        )
    }

    pub fn new_with_surface_limits(
        max_sessions: usize,
        retention_per_session: usize,
        surface_limits: SurfaceLimits,
    ) -> Self {
        Self {
            registry: LiveSessionRegistry::new(max_sessions),
            routing: RwLock::new(RoutingState::new_with_limits(
                retention_per_session,
                surface_limits,
            )),
            activity: SessionActivityTracker::default(),
            server_request_waiters: std::sync::Mutex::new(std::collections::HashMap::new()),
            owner_tasks: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn registry(&self) -> &LiveSessionRegistry<H> {
        &self.registry
    }

    /// Resolve one explicitly targeted interactive capability while holding
    /// the registry/routing locks in their canonical order. The returned
    /// handle is opaque to AppServer and remains subject to its own draining
    /// state after this short validation section ends.
    pub fn validate_interactive_target(
        &self,
        connection: ConnectionKey,
        target: &InteractiveTarget,
    ) -> Result<ValidatedInteractiveSession<H>, AppServerError> {
        let sessions = self
            .registry
            .sessions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let routing = self
            .routing
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let attachment = routing
            .surface_attachment(&target.surface_id)
            .ok_or_else(|| {
                CallingSurfaceNotAttachedSnafu {
                    surface_id: target.surface_id.clone(),
                }
                .build()
            })?;
        if attachment.connection != connection {
            return CallingSurfaceWrongConnectionSnafu {
                surface_id: target.surface_id.clone(),
            }
            .fail();
        }
        if attachment.state != SurfaceState::Attached || attachment.role != SurfaceRole::Interactive
        {
            return CallingSurfaceNotInteractiveSnafu {
                surface_id: target.surface_id.clone(),
            }
            .fail();
        }
        if attachment.session_id != target.session_id {
            return CallingSurfaceWrongSessionSnafu {
                surface_id: target.surface_id.clone(),
                expected_session_id: target.session_id.clone(),
                actual_session_id: attachment.session_id.clone(),
            }
            .fail();
        }
        let handle = match sessions.get(&target.session_id) {
            Some(SessionSlot::Live(handle)) => handle.clone(),
            Some(SessionSlot::Loading(_)) => {
                return TargetSessionNotLiveSnafu {
                    session_id: target.session_id.clone(),
                    state: "loading",
                }
                .fail();
            }
            Some(SessionSlot::Closing(_)) => {
                return TargetSessionNotLiveSnafu {
                    session_id: target.session_id.clone(),
                    state: "closing",
                }
                .fail();
            }
            None => {
                return TargetSessionNotLiveSnafu {
                    session_id: target.session_id.clone(),
                    state: "missing",
                }
                .fail();
            }
        };
        Ok(ValidatedInteractiveSession {
            handle,
            attachment: attachment.clone(),
        })
    }

    pub fn validate_orphan_close_target(
        &self,
        session_id: &SessionId,
    ) -> Result<H, AppServerError> {
        let sessions = self
            .registry
            .sessions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let routing = self
            .routing
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if routing.interactive_owner(session_id).is_some() {
            return InteractiveOwnerConflictSnafu {
                session_id: session_id.clone(),
            }
            .fail();
        }
        match sessions.get(session_id) {
            Some(SessionSlot::Live(handle)) => Ok(handle.clone()),
            Some(SessionSlot::Loading(_)) => TargetSessionNotLiveSnafu {
                session_id: session_id.clone(),
                state: "loading",
            }
            .fail(),
            Some(SessionSlot::Closing(_)) => TargetSessionNotLiveSnafu {
                session_id: session_id.clone(),
                state: "closing",
            }
            .fail(),
            None => TargetSessionNotLiveSnafu {
                session_id: session_id.clone(),
                state: "missing",
            }
            .fail(),
        }
    }

    pub fn routing(&self) -> &RwLock<RoutingState> {
        &self.routing
    }

    pub fn commit_replace_for_surface(
        &self,
        old_session_id: &SessionId,
        new_session_id: &SessionId,
        new_handle: H,
        calling_surface: &SurfaceId,
    ) -> Result<AppReplaceCommit<H>, ReplaceCommitFailure<H>> {
        let mut sessions = self
            .registry
            .sessions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Every fallible step happens before `new_handle` is consumed, so on
        // failure the un-committed handle is handed back to the caller for
        // teardown — dropping it here would leak the fully-constructed runtime
        // (SessionEnd hooks never fire, its shutdown token never cancels, and
        // its session tasks never exit → an unbounded runtime/task leak).
        let old_handle = match sessions.get(old_session_id) {
            Some(SessionSlot::Live(handle)) => handle.clone(),
            _ => {
                let error = RegistrySnafu.into_error(
                    crate::registry::OldNotReadySnafu {
                        session_id: old_session_id.clone(),
                    }
                    .build(),
                );
                return Err(ReplaceCommitFailure::new(error, new_handle));
            }
        };
        if !matches!(sessions.get(new_session_id), Some(SessionSlot::Loading(_))) {
            let error = RegistrySnafu.into_error(
                crate::registry::SlotConflictSnafu {
                    session_id: new_session_id.clone(),
                    expected: "Loading",
                }
                .build(),
            );
            return Err(ReplaceCommitFailure::new(error, new_handle));
        }

        let mut routing = self
            .routing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(actual_session_id) = routing.surface_session(calling_surface).cloned() else {
            let error = CallingSurfaceNotAttachedSnafu {
                surface_id: calling_surface.clone(),
            }
            .build();
            return Err(ReplaceCommitFailure::new(error, new_handle));
        };
        if &actual_session_id != old_session_id {
            let error = CallingSurfaceWrongSessionSnafu {
                surface_id: calling_surface.clone(),
                expected_session_id: old_session_id.clone(),
                actual_session_id,
            }
            .build();
            return Err(ReplaceCommitFailure::new(error, new_handle));
        }

        // Everything validated — mutate with no further fallible step.
        let old_close = CloseState::new();
        let old_close_completion = old_close.completion();
        // Consume the new reservation so a close-after-load recorded on it is
        // honored: the slot promotes to `Live`, or straight to `Closing`
        // if a shutdown raced the replace.
        let Some(SessionSlot::Loading(new_load)) = sessions.remove(new_session_id) else {
            unreachable!("new slot was matched as Loading above");
        };
        sessions.insert(new_session_id.clone(), new_load.promote(new_handle));
        sessions.insert(
            old_session_id.clone(),
            SessionSlot::Closing(ClosingState {
                handle: old_handle.clone(),
                close: old_close,
            }),
        );

        // The calling surface was validated against `old_session_id` above,
        // in this same routing-lock section, so the re-point cannot miss.
        #[expect(
            clippy::expect_used,
            reason = "calling surface validated under the routing lock above"
        )]
        let routing_outcome = routing
            .replace_calling_surface(calling_surface, new_session_id.clone())
            .expect("calling surface was validated under the routing lock");
        self.cancel_server_request_waiters(&routing_outcome.cancelled_requests);
        let lifecycle_effects = replace_lifecycle_effects(&routing_outcome);
        self.activity.touch(new_session_id.clone());

        Ok(AppReplaceCommit {
            old_handle,
            old_close_completion,
            routing_outcome,
            lifecycle_effects,
        })
    }

    /// Atomically repoint a source interactive surface to an already-live,
    /// orphaned destination and move the source session into `Closing`.
    pub fn commit_replace_to_live_for_surface(
        &self,
        old_session_id: &SessionId,
        new_session_id: &SessionId,
        calling_surface: &SurfaceId,
    ) -> Result<AppReplaceCommit<H>, AppServerError> {
        let mut sessions = self
            .registry
            .sessions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let old_handle = match sessions.get(old_session_id) {
            Some(SessionSlot::Live(handle)) => handle.clone(),
            _ => {
                return crate::registry::OldNotReadySnafu {
                    session_id: old_session_id.clone(),
                }
                .fail()
                .context(RegistrySnafu);
            }
        };
        if !matches!(sessions.get(new_session_id), Some(SessionSlot::Live(_))) {
            return crate::registry::SlotConflictSnafu {
                session_id: new_session_id.clone(),
                expected: "Live",
            }
            .fail()
            .context(RegistrySnafu);
        }

        let mut routing = self
            .routing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(actual_session_id) = routing.surface_session(calling_surface).cloned() else {
            return CallingSurfaceNotAttachedSnafu {
                surface_id: calling_surface.clone(),
            }
            .fail();
        };
        if &actual_session_id != old_session_id {
            return CallingSurfaceWrongSessionSnafu {
                surface_id: calling_surface.clone(),
                expected_session_id: old_session_id.clone(),
                actual_session_id,
            }
            .fail();
        }
        if routing.interactive_owner(new_session_id).is_some() {
            return InteractiveOwnerConflictSnafu {
                session_id: new_session_id.clone(),
            }
            .fail();
        }

        let old_close = CloseState::new();
        let old_close_completion = old_close.completion();
        sessions.insert(
            old_session_id.clone(),
            SessionSlot::Closing(ClosingState {
                handle: old_handle.clone(),
                close: old_close,
            }),
        );
        #[expect(
            clippy::expect_used,
            reason = "calling surface validated under the routing lock above"
        )]
        let routing_outcome = routing
            .replace_calling_surface(calling_surface, new_session_id.clone())
            .expect("calling surface was validated under the routing lock");
        self.cancel_server_request_waiters(&routing_outcome.cancelled_requests);
        let lifecycle_effects = replace_lifecycle_effects(&routing_outcome);
        self.activity.touch(new_session_id.clone());
        Ok(AppReplaceCommit {
            old_handle,
            old_close_completion,
            routing_outcome,
            lifecycle_effects,
        })
    }

    pub fn complete_session_close(
        &self,
        session_id: &SessionId,
        close_result: Result<(), RegistryError>,
    ) -> Result<AppCloseCommit, AppServerError> {
        let mut sessions = self
            .registry
            .sessions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let close_sender = match sessions.get(session_id) {
            Some(SessionSlot::Closing(closing)) => closing.close.sender.clone(),
            _ => {
                return crate::registry::SlotConflictSnafu {
                    session_id: session_id.clone(),
                    expected: "Closing",
                }
                .fail()
                .context(RegistrySnafu);
            }
        };

        let mut routing = self
            .routing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let routing_outcome = routing.close_session_surfaces(session_id);
        self.cancel_server_request_waiters(&routing_outcome.cancelled_requests);
        let lifecycle_effects = close_lifecycle_effects(session_id, &routing_outcome);
        let _ = close_sender.send(Some(close_result));
        sessions.remove(session_id);
        self.activity.forget(session_id);

        Ok(AppCloseCommit {
            routing_outcome,
            lifecycle_effects,
        })
    }

    pub fn resolve_server_request(
        &self,
        target: &InteractiveTarget,
        reply: ServerRequestReply,
    ) -> Result<ResolvedServerRequest, AppServerError> {
        let request_id = RequestId::String(reply.request_id().to_string());
        let mut routing = self
            .routing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pending = routing
            .complete_server_request(&request_id, target)
            .map_err(AppServerError::from)?;
        drop(routing);
        if let Some(waiter) = self
            .server_request_waiters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&request_id)
        {
            let _ = waiter.send(reply.clone());
        }
        Ok(ResolvedServerRequest { pending, reply })
    }

    pub fn resolve_server_request_by_id(
        &self,
        request_id: &RequestId,
        reply: ServerRequestReply,
    ) -> Result<ResolvedServerRequest, AppServerError> {
        let mut routing = self
            .routing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pending = routing
            .complete_server_request_by_id(request_id)
            .map_err(AppServerError::from)?;
        drop(routing);
        if let Some(waiter) = self
            .server_request_waiters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(request_id)
        {
            let _ = waiter.send(reply.clone());
        }
        Ok(ResolvedServerRequest { pending, reply })
    }

    pub fn cancel_server_request_for_connection(
        &self,
        connection: ConnectionKey,
        request_id: &RequestId,
    ) -> Result<PendingServerRequest, AppServerError> {
        let mut routing = self
            .routing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pending = routing
            .cancel_server_request_for_connection(request_id, connection)
            .map_err(AppServerError::from)?;
        drop(routing);
        self.cancel_server_request_waiters(std::slice::from_ref(request_id));
        Ok(pending)
    }

    pub fn connect_with_request_and_lifecycle_senders(
        &self,
        connection: ConnectionKey,
        sender: OutboundSender,
        request_sender: ServerRequestSender,
        lifecycle_sender: SurfaceLifecycleSender,
    ) {
        self.routing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .connect_with_request_and_lifecycle_senders(
                connection,
                sender,
                request_sender,
                lifecycle_sender,
            );
    }

    pub fn attach_surface_with_options(
        &self,
        connection: ConnectionKey,
        surface_id: SurfaceId,
        session_id: SessionId,
        options: AttachSurfaceOptions,
    ) -> Result<(), AttachError> {
        let result = self
            .routing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .attach_surface_with_options(connection, surface_id, session_id.clone(), options);
        if result.is_ok() {
            self.activity.touch(session_id);
        }
        result
    }

    /// Attach a surface only if the target session is `Live` in the registry.
    ///
    /// Rejects `Closing` (`SessionClosing`) and missing / `Loading`
    /// (`SessionNotFound`), so a client cannot silently attach to a dead or
    /// not-yet-live session and then hang forever with no events and no
    /// lifecycle effect. The registry read guard is held across the routing
    /// attach (registry -> routing order) so a concurrent close cannot orphan
    /// the freshly attached surface between the check and the attach.
    pub fn attach_live_surface_with_options(
        &self,
        connection: ConnectionKey,
        surface_id: SurfaceId,
        session_id: SessionId,
        options: AttachSurfaceOptions,
    ) -> Result<(), AttachError> {
        let registry = self
            .registry
            .sessions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::ensure_live_slot(&registry, &session_id)?;
        let result = self
            .routing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .attach_surface_with_options(connection, surface_id, session_id.clone(), options);
        drop(registry);
        if result.is_ok() {
            self.activity.touch(session_id);
        }
        result
    }

    /// Registry-slot guard shared by the live attach/subscribe paths.
    fn ensure_live_slot(
        sessions: &std::collections::HashMap<SessionId, SessionSlot<H>>,
        session_id: &SessionId,
    ) -> Result<(), AttachError> {
        match sessions.get(session_id) {
            Some(SessionSlot::Live(_)) => Ok(()),
            Some(SessionSlot::Closing(_)) => crate::SessionClosingSnafu {
                session_id: session_id.clone(),
            }
            .fail(),
            Some(SessionSlot::Loading(_)) | None => crate::SessionNotFoundSnafu {
                session_id: session_id.clone(),
            }
            .fail(),
        }
    }

    pub fn subscribe_surface_with_options(
        &self,
        connection: ConnectionKey,
        surface_id: SurfaceId,
        session_id: SessionId,
        after_seq: Option<i64>,
        options: AttachSurfaceOptions,
    ) -> Result<SubscribeReplay, AttachError> {
        let result = self
            .routing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .subscribe_with_options(
                connection,
                surface_id,
                session_id.clone(),
                after_seq,
                options,
            );
        if result.is_ok() {
            self.activity.touch(session_id);
        }
        result
    }

    /// Passive-subscribe a surface only if the target session is `Live` in the
    /// registry. See [`AppServer::attach_live_surface_with_options`]: this is
    /// the guard that stops a `session/subscribe` for a missing, `Loading`, or
    /// already-closed session from returning a surface that never receives an
    /// event, a lifecycle effect, or an error.
    pub fn subscribe_live_surface_with_options(
        &self,
        connection: ConnectionKey,
        surface_id: SurfaceId,
        session_id: SessionId,
        after_seq: Option<i64>,
        options: AttachSurfaceOptions,
    ) -> Result<SubscribeReplay, AttachError> {
        let registry = self
            .registry
            .sessions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::ensure_live_slot(&registry, &session_id)?;
        let result = self
            .routing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .subscribe_with_options(
                connection,
                surface_id,
                session_id.clone(),
                after_seq,
                options,
            );
        drop(registry);
        if result.is_ok() {
            self.activity.touch(session_id);
        }
        result
    }

    pub fn route_envelope(&self, envelope: SessionEnvelope) -> RouteOutcome {
        let session_id = envelope.session_id.clone();
        let outcome = self
            .routing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .route_envelope(envelope);
        // A full/closed consumer disconnected mid-route; resolve any server
        // request waiters it owned so an in-flight turn is not wedged. The
        // routing write guard above is a temporary, already dropped, so taking
        // the waiters lock here keeps the registry->routing->waiters order.
        self.cancel_server_request_waiters(&outcome.cancelled_requests);
        self.activity.touch(session_id);
        outcome
    }

    /// Seed a resumed session's retention-ring high-water from its seq
    /// skip-ahead. Callers pair this with
    /// `SessionSeqAllocator::initialize_after_watermark` so an empty ring
    /// rejects a stale cursor.
    pub fn initialize_session_ring_watermark(&self, session_id: SessionId, high_seq: i64) {
        self.routing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .initialize_ring_watermark(session_id, high_seq);
    }

    pub fn disconnect(&self, connection: ConnectionKey) -> DisconnectOutcome {
        let mut routing = self
            .routing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let session_ids = routing.connection_session_ids(connection);
        let outcome = routing.disconnect(connection);
        self.cancel_server_request_waiters(&outcome.cancelled_requests);
        drop(routing);
        for session_id in session_ids {
            self.activity.touch(session_id);
        }
        outcome
    }

    pub fn detach_surface_for_connection(
        &self,
        connection: ConnectionKey,
        surface_id: &SurfaceId,
    ) -> DetachSurfaceOutcome {
        let mut routing = self
            .routing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let session_id = routing.surface_session(surface_id).cloned();
        let outcome = routing.detach_surface_for_connection(connection, surface_id);
        self.cancel_server_request_waiters(&outcome.cancelled_requests);
        drop(routing);
        if outcome.detached_surface.is_some()
            && let Some(session_id) = session_id
        {
            self.activity.touch(session_id);
        }
        outcome
    }

    pub fn touch_session_activity(&self, session_id: SessionId) {
        self.activity.touch(session_id);
    }

    pub fn session_last_activity(&self, session_id: &SessionId) -> Option<Instant> {
        self.activity.last_activity(session_id)
    }

    pub fn subscribe_session_activity(&self) -> tokio::sync::watch::Receiver<u64> {
        self.activity.subscribe()
    }

    pub fn list_live_sessions(&self) -> Vec<AppLiveSessionSummary> {
        let sessions = self
            .registry
            .sessions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let routing = self
            .routing
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut summaries = sessions
            .iter()
            .filter(|&(_, slot)| matches!(slot, SessionSlot::Live(_)))
            .map(|(session_id, _)| AppLiveSessionSummary {
                session_id: session_id.clone(),
                surface_counts: routing.surface_counts_for_session(session_id),
            })
            .collect::<Vec<_>>();
        summaries.sort_by(|a, b| a.session_id.as_str().cmp(b.session_id.as_str()));
        summaries
    }

    pub fn has_session_slot(&self, session_id: &SessionId) -> bool {
        self.registry
            .sessions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(session_id)
    }

    /// Session ids that must remain announced to process egress: Live plus
    /// retiring Closing slots whose final `SessionResult` may still be in
    /// flight. Used by Event Hub membership so a closing session is not dropped
    /// before its final local-egress handoff completes (CS-4 / R17).
    pub fn announced_session_ids(&self) -> Vec<SessionId> {
        self.registry.list_announced()
    }

    pub fn route_server_request(
        &self,
        session_id: SessionId,
        capability: SurfaceCapability,
        turn_id: Option<TurnId>,
        request: ServerRequest,
    ) -> Result<ServerRequestRouteOutcome, ServerRequestRouteError> {
        let result = self
            .routing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .route_server_request(session_id, capability, turn_id, request);
        self.cancel_route_error_waiters(result.as_ref().err());
        result
    }

    pub fn route_server_request_with_reply(
        &self,
        session_id: SessionId,
        capability: SurfaceCapability,
        turn_id: Option<TurnId>,
        request: ServerRequest,
    ) -> Result<tokio::sync::oneshot::Receiver<ServerRequestReply>, ServerRequestRouteError> {
        let outcome = {
            let mut routing = self
                .routing
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            routing.route_server_request(session_id, capability, turn_id, request)
        };
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                self.cancel_route_error_waiters(Some(&error));
                return Err(error);
            }
        };
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.server_request_waiters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(outcome.pending.request_id, sender);
        Ok(receiver)
    }

    /// Resolve waiters for requests cancelled by a queue-full disconnect that
    /// happened while routing a different server request. The routing lock must
    /// already be released before this is called (waiters lock nests under it).
    fn cancel_route_error_waiters(&self, error: Option<&ServerRequestRouteError>) {
        if let Some(ServerRequestRouteError::QueueUnavailable {
            cancelled_requests, ..
        }) = error
        {
            self.cancel_server_request_waiters(cancelled_requests);
        }
    }

    /// Cancel every pending server->client request scoped to `turn_id`, removing
    /// its routing bookkeeping and resolving its waiter. Called when a turn ends
    /// so an interrupted turn's outstanding approval/hook/MCP requests do not
    /// leak their pending entries + retained payloads until the surface or
    /// session goes away.
    pub fn cancel_turn_server_requests(&self, turn_id: &TurnId) {
        let cancelled = self
            .routing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .cancel_turn_server_requests(turn_id);
        self.cancel_server_request_waiters(&cancelled);
    }

    fn cancel_server_request_waiters(&self, request_ids: &[RequestId]) {
        if request_ids.is_empty() {
            return;
        }
        let mut waiters = self
            .server_request_waiters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for request_id in request_ids {
            waiters.remove(request_id);
        }
    }

    pub fn pending_server_request_replays_for_surface(
        &self,
        surface_id: &SurfaceId,
    ) -> Vec<PendingServerRequestReplay> {
        self.routing
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending_server_request_replays_for_surface(surface_id)
    }

    pub fn route_lifecycle_effects(
        &self,
        effects: Vec<SurfaceLifecycleEffect>,
    ) -> LifecycleRouteOutcome {
        let outcome = self
            .routing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .route_lifecycle_effects(effects);
        self.cancel_server_request_waiters(&outcome.cancelled_requests);
        outcome
    }
}

#[derive(Debug, Clone)]
pub struct ValidatedInteractiveSession<H> {
    pub handle: H,
    pub attachment: SurfaceAttachment,
}

impl<H> AppServer<H>
where
    H: Clone + Send + Sync + 'static,
{
    fn track_owner_task(&self, handle: tokio::task::JoinHandle<()>) {
        let mut tasks = self
            .owner_tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        tasks.retain(|task| !task.is_finished());
        tasks.push(handle);
    }

    /// Spawn a lifecycle owner task and retain its join handle for shutdown.
    fn spawn_tracked<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let handle = tokio::spawn(future);
        self.track_owner_task(handle);
    }

    /// Abort and join any lifecycle owner tasks still in flight. Process
    /// shutdown calls this after per-session closes complete, so no owner task
    /// is left detached past the shutdown deadline (CS-3c).
    pub async fn abort_and_join_owner_tasks(&self) {
        let tasks = {
            let mut guard = self
                .owner_tasks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::take(&mut *guard)
        };
        for task in tasks {
            if !task.is_finished() {
                task.abort();
            }
            let _ = task.await;
        }
    }

    pub fn spawn_load<F>(
        self: &Arc<Self>,
        session_id: SessionId,
        factory: F,
    ) -> Result<AppLoadStart<H>, AppServerError>
    where
        F: Future<Output = Result<H, RegistryError>> + Send + 'static,
    {
        match self
            .registry
            .begin_load(session_id.clone())
            .context(RegistrySnafu)?
        {
            LoadStart::Reserved => {
                let LoadStart::Loading(completion) = self
                    .registry
                    .begin_load(session_id.clone())
                    .context(RegistrySnafu)?
                else {
                    unreachable!("reserved load must be observable as Loading");
                };
                let server = Arc::clone(self);
                self.spawn_tracked(async move {
                    let mut guard = OwnerGuard::new(
                        Arc::clone(&server),
                        OwnerGuardAction::FailLoad(session_id.clone()),
                    );
                    match factory.await {
                        Ok(handle) => {
                            if server
                                .registry
                                .complete_load_success(&session_id, handle)
                                .is_ok()
                            {
                                server.activity.touch(session_id.clone());
                            }
                        }
                        Err(error) => {
                            let _ = server.registry.complete_load_failure(&session_id, error);
                        }
                    }
                    guard.disarm();
                });
                Ok(AppLoadStart::Started { completion })
            }
            LoadStart::Live(handle) => {
                self.activity.touch(session_id);
                Ok(AppLoadStart::Live(handle))
            }
            LoadStart::Loading(completion) => Ok(AppLoadStart::Loading(completion)),
            LoadStart::Closing(completion) => Ok(AppLoadStart::Closing(completion)),
        }
    }

    pub fn spawn_close<C, Fut>(
        self: &Arc<Self>,
        session_id: SessionId,
        close: C,
    ) -> Result<AppCloseStart, AppServerError>
    where
        C: FnOnce(H) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), RegistryError>> + Send + 'static,
    {
        match self
            .registry
            .begin_close(&session_id)
            .context(RegistrySnafu)?
        {
            CloseStart::Started { handle, completion } => {
                let server = Arc::clone(self);
                self.spawn_tracked(async move {
                    let mut guard = OwnerGuard::new(
                        Arc::clone(&server),
                        OwnerGuardAction::Close(session_id.clone()),
                    );
                    let close_result = close(handle).await;
                    if let Ok(commit) = server.complete_session_close(&session_id, close_result) {
                        server.route_lifecycle_effects(commit.lifecycle_effects);
                    }
                    guard.disarm();
                });
                Ok(AppCloseStart::Started { completion })
            }
            CloseStart::Loading {
                mut load_completion,
                close_completion,
                should_spawn,
            } => {
                if should_spawn {
                    let server = Arc::clone(self);
                    let close_session_id = session_id.clone();
                    self.spawn_tracked(async move {
                        // Not guarded during the load wait: a load failure fires
                        // the close signal and removes the slot (there is nothing
                        // to close). Guard only the actual cascade below.
                        if let Ok(handle) = load_completion.wait().await {
                            let mut guard = OwnerGuard::new(
                                Arc::clone(&server),
                                OwnerGuardAction::Close(close_session_id.clone()),
                            );
                            let close_result = close(handle).await;
                            if let Ok(commit) =
                                server.complete_session_close(&close_session_id, close_result)
                            {
                                server.route_lifecycle_effects(commit.lifecycle_effects);
                            }
                            guard.disarm();
                        }
                    });
                }
                Ok(AppCloseStart::Loading(close_completion))
            }
            CloseStart::Closing { completion, .. } => Ok(AppCloseStart::Closing(completion)),
        }
    }

    pub fn spawn_close_orphan<C, Fut>(
        self: &Arc<Self>,
        session_id: SessionId,
        close: C,
    ) -> Result<AppCloseStart, AppServerError>
    where
        C: FnOnce(H) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), RegistryError>> + Send + 'static,
    {
        let (handle, completion) = {
            let mut sessions = self
                .registry
                .sessions
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let routing = self
                .routing
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if routing.interactive_owner(&session_id).is_some() {
                return InteractiveOwnerConflictSnafu { session_id }.fail();
            }
            let handle = match sessions.remove(&session_id) {
                Some(SessionSlot::Live(handle)) => handle,
                Some(slot) => {
                    sessions.insert(session_id.clone(), slot);
                    return TargetSessionNotLiveSnafu {
                        session_id: session_id.clone(),
                        state: "not_live",
                    }
                    .fail();
                }
                None => {
                    return TargetSessionNotLiveSnafu {
                        session_id: session_id.clone(),
                        state: "missing",
                    }
                    .fail();
                }
            };
            let close_state = CloseState::new();
            let completion = close_state.completion();
            sessions.insert(
                session_id.clone(),
                SessionSlot::Closing(ClosingState {
                    handle: handle.clone(),
                    close: close_state,
                }),
            );
            (handle, completion)
        };

        let server = Arc::clone(self);
        self.spawn_tracked(async move {
            let mut guard = OwnerGuard::new(
                Arc::clone(&server),
                OwnerGuardAction::Close(session_id.clone()),
            );
            let close_result = close(handle).await;
            if let Ok(commit) = server.complete_session_close(&session_id, close_result) {
                server.route_lifecycle_effects(commit.lifecycle_effects);
            }
            guard.disarm();
        });
        Ok(AppCloseStart::Started { completion })
    }

    pub fn spawn_replace<F, Close, CloseFut>(
        self: &Arc<Self>,
        old_session_id: SessionId,
        new_session_id: SessionId,
        calling_surface: SurfaceId,
        factory: F,
        // Runs the close cascade for a handle, deriving its target from the
        // handle itself. Invoked on the OLD handle after a successful commit,
        // or on the NEW handle to tear it down when the commit fails.
        close_handle: Close,
    ) -> Result<AppReplaceStart<H>, AppServerError>
    where
        F: Future<Output = Result<H, RegistryError>> + Send + 'static,
        Close: FnOnce(H) -> CloseFut + Send + 'static,
        CloseFut: Future<Output = Result<(), RegistryError>> + Send + 'static,
    {
        let ReplaceStart::Reserved { new_completion, .. } = self
            .registry
            .begin_replace(&old_session_id, new_session_id.clone())
            .context(RegistrySnafu)?;
        let server = Arc::clone(self);
        self.spawn_tracked(async move {
            let mut guard = OwnerGuard::new(
                Arc::clone(&server),
                OwnerGuardAction::FailLoad(new_session_id.clone()),
            );
            match factory.await {
                Ok(new_handle) => {
                    match server.commit_replace_for_surface(
                        &old_session_id,
                        &new_session_id,
                        new_handle,
                        &calling_surface,
                    ) {
                        Ok(commit) => {
                            // Committed: new is live, old is Closing. The hazard
                            // now is a panic in the old close cascade wedging old.
                            guard.arm_close(old_session_id.clone());
                            server.route_lifecycle_effects(commit.lifecycle_effects);
                            let close_result = close_handle(commit.old_handle).await;
                            if let Ok(close_commit) =
                                server.complete_session_close(&old_session_id, close_result)
                            {
                                server.route_lifecycle_effects(close_commit.lifecycle_effects);
                            }
                            guard.disarm();
                        }
                        Err(failure) => {
                            // Commit failed after the factory built a full
                            // runtime (e.g. the calling surface disconnected
                            // mid-construction). Tear the new runtime down via
                            // the same close cascade so its SessionEnd hooks
                            // fire and its session tasks are cancelled/joined —
                            // dropping it would leak the runtime and its tasks
                            // for the process lifetime.
                            tracing::warn!(
                                error = %failure.error,
                                new_session_id = %new_session_id,
                                "replace commit failed; tearing down the constructed runtime"
                            );
                            let _ = close_handle(failure.handle).await;
                            let _ = server.registry.complete_replace_failure(
                                &new_session_id,
                                crate::registry::SlotConflictSnafu {
                                    session_id: new_session_id.clone(),
                                    expected: "ReplaceCommit",
                                }
                                .build(),
                            );
                            guard.disarm();
                        }
                    }
                }
                Err(error) => {
                    let _ = server
                        .registry
                        .complete_replace_failure(&new_session_id, error);
                    guard.disarm();
                }
            }
        });
        Ok(AppReplaceStart::Started {
            completion: new_completion,
        })
    }

    /// Owner-task variant of the replace-to-already-live-orphan commit.
    ///
    /// Repoints the calling interactive surface from `old_session_id` to the
    /// already-live `new_session_id`, moves the source into `Closing`, then runs
    /// the supplied source close cascade in a tracked owner task under an
    /// `OwnerGuard`. This is the surface-aware sibling of `spawn_replace` for the
    /// case where the destination already exists. Hosts must route through this
    /// rather than hand-rolling the source close in a bare `tokio::spawn`: a
    /// panic there would wedge the source in `Closing` forever (every close
    /// waiter hangs, and the slot permanently consumes a `max_sessions` unit),
    /// and a bare spawn is not tracked for shutdown joining.
    pub fn spawn_replace_to_live<Close, CloseFut>(
        self: &Arc<Self>,
        old_session_id: SessionId,
        new_session_id: SessionId,
        calling_surface: SurfaceId,
        close_old: Close,
    ) -> Result<CloseCompletion, AppServerError>
    where
        Close: FnOnce(H) -> CloseFut + Send + 'static,
        CloseFut: Future<Output = Result<(), RegistryError>> + Send + 'static,
    {
        let commit = self.commit_replace_to_live_for_surface(
            &old_session_id,
            &new_session_id,
            &calling_surface,
        )?;
        let completion = commit.old_close_completion.clone();
        // Emit session/started (caller) + session/replaced (old peers) before
        // the source close cascade runs.
        self.route_lifecycle_effects(commit.lifecycle_effects);
        let server = Arc::clone(self);
        let old_handle = commit.old_handle;
        self.spawn_tracked(async move {
            let mut guard = OwnerGuard::new(
                Arc::clone(&server),
                OwnerGuardAction::Close(old_session_id.clone()),
            );
            let close_result = close_old(old_handle).await;
            if let Ok(close_commit) = server.complete_session_close(&old_session_id, close_result) {
                server.route_lifecycle_effects(close_commit.lifecycle_effects);
            }
            guard.disarm();
        });
        Ok(completion)
    }

    pub fn spawn_shutdown<C, Fut>(self: &Arc<Self>, close: C) -> AppShutdownStart
    where
        C: Fn(H) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = Result<(), RegistryError>> + Send + 'static,
    {
        let mut sessions = Vec::new();
        let mut errors = Vec::new();
        for session_id in self.registry.list_closable() {
            match self.spawn_close(session_id.clone(), close.clone()) {
                Ok(start) => sessions.push(AppShutdownSession {
                    session_id,
                    completion: start.completion(),
                }),
                Err(error) => errors.push((session_id, error)),
            }
        }
        AppShutdownStart { sessions, errors }
    }
}

/// Completes a wedged slot if an owner task unwinds or is aborted before it
/// finishes normally. Without this, a factory/cascade panic would leave
/// the completion sender alive inside the slot: every waiter hangs forever and
/// the slot permanently consumes a `max_sessions` unit (fatal at
/// `max_sessions = 1`). Owner tasks arm it around the panic-prone work and
/// `disarm()` on the normal path; `arm_close` switches the action once a
/// replace has committed and the hazard moves from the new reservation to the
/// old session's close cascade.
struct OwnerGuard<H: Clone + Send + Sync + 'static> {
    server: Arc<AppServer<H>>,
    action: Option<OwnerGuardAction>,
}

enum OwnerGuardAction {
    FailLoad(SessionId),
    Close(SessionId),
}

impl<H: Clone + Send + Sync + 'static> OwnerGuard<H> {
    fn new(server: Arc<AppServer<H>>, action: OwnerGuardAction) -> Self {
        Self {
            server,
            action: Some(action),
        }
    }

    fn disarm(&mut self) {
        self.action = None;
    }

    fn arm_close(&mut self, session_id: SessionId) {
        self.action = Some(OwnerGuardAction::Close(session_id));
    }
}

impl<H: Clone + Send + Sync + 'static> Drop for OwnerGuard<H> {
    fn drop(&mut self) {
        match self.action.take() {
            Some(OwnerGuardAction::FailLoad(session_id)) => {
                let _ = self.server.registry.complete_load_failure(
                    &session_id,
                    crate::registry::RegistryError::load_failed(
                        "owner task aborted before completion",
                    ),
                );
            }
            Some(OwnerGuardAction::Close(session_id)) => {
                let _ = self.server.complete_session_close(
                    &session_id,
                    Err(crate::registry::RegistryError::close_failed(
                        "owner task aborted before completion",
                    )),
                );
            }
            None => {}
        }
    }
}

fn replace_lifecycle_effects(outcome: &ReplaceSurfaceOutcome) -> Vec<SurfaceLifecycleEffect> {
    let mut effects = Vec::with_capacity(outcome.detached_surfaces.len() + 1);
    effects.push(SurfaceLifecycleEffect {
        surface_id: outcome.calling_surface.clone(),
        kind: SurfaceLifecycleEffectKind::SessionStarted {
            session_id: outcome.new_session_id.clone(),
        },
    });
    effects.extend(outcome.detached_surfaces.iter().cloned().map(|surface_id| {
        SurfaceLifecycleEffect {
            surface_id,
            kind: SurfaceLifecycleEffectKind::SessionReplaced {
                old_session_id: outcome.old_session_id.clone(),
                new_session_id: outcome.new_session_id.clone(),
            },
        }
    }));
    effects
}

fn close_lifecycle_effects(
    session_id: &SessionId,
    outcome: &CloseSessionSurfacesOutcome,
) -> Vec<SurfaceLifecycleEffect> {
    outcome
        .closed_surfaces
        .iter()
        .cloned()
        .map(|surface_id| SurfaceLifecycleEffect {
            surface_id,
            kind: SurfaceLifecycleEffectKind::SessionEnded {
                session_id: session_id.clone(),
            },
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct AppReplaceCommit<H> {
    pub old_handle: H,
    pub old_close_completion: CloseCompletion,
    pub routing_outcome: ReplaceSurfaceOutcome,
    pub lifecycle_effects: Vec<SurfaceLifecycleEffect>,
}

/// Failure of `commit_replace_for_surface` that hands the un-committed new
/// handle back to the caller so it can run the runtime teardown (fire
/// SessionEnd hooks, cancel the shutdown token, join session tasks). A plain
/// struct, not a snafu enum: the generic `H` is not an `Error`, and this is a
/// by-value control-flow carrier that is matched, never `?`-propagated.
#[derive(Debug)]
pub struct ReplaceCommitFailure<H> {
    pub error: AppServerError,
    pub handle: H,
}

impl<H> ReplaceCommitFailure<H> {
    fn new(error: AppServerError, handle: H) -> Self {
        Self { error, handle }
    }
}

#[derive(Debug, Clone)]
pub struct AppCloseCommit {
    pub routing_outcome: CloseSessionSurfacesOutcome,
    pub lifecycle_effects: Vec<SurfaceLifecycleEffect>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppLiveSessionSummary {
    pub session_id: SessionId,
    pub surface_counts: SessionSurfaceCounts,
}

#[derive(Debug, Clone)]
pub enum AppLoadStart<H> {
    Started { completion: LoadCompletion<H> },
    Live(H),
    Loading(LoadCompletion<H>),
    Closing(CloseCompletion),
}

#[derive(Debug, Clone)]
pub enum AppCloseStart {
    Started { completion: CloseCompletion },
    Loading(CloseCompletion),
    Closing(CloseCompletion),
}

impl AppCloseStart {
    pub fn completion(&self) -> CloseCompletion {
        match self {
            Self::Started { completion }
            | Self::Loading(completion)
            | Self::Closing(completion) => completion.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum AppReplaceStart<H> {
    Started { completion: LoadCompletion<H> },
}

#[derive(Debug)]
pub struct AppShutdownStart {
    pub sessions: Vec<AppShutdownSession>,
    pub errors: Vec<(SessionId, AppServerError)>,
}

#[derive(Debug, Clone)]
pub struct AppShutdownSession {
    pub session_id: SessionId,
    pub completion: CloseCompletion,
}

#[derive(Debug, Clone)]
pub struct ResolvedServerRequest {
    pub pending: PendingServerRequest,
    pub reply: ServerRequestReply,
}

#[derive(Debug, Clone)]
pub enum ServerRequestReply {
    Approval(ApprovalResolveParams),
    UserInput(UserInputResolveParams),
    Elicitation(ElicitationResolveParams),
    McpRouteMessage {
        request_id: String,
        result: serde_json::Value,
    },
    HookCallback {
        request_id: String,
        result: serde_json::Value,
    },
    Error(ServerRequestErrorReply),
}

impl ServerRequestReply {
    pub fn request_id(&self) -> &str {
        match self {
            Self::Approval(params) => &params.request_id,
            Self::UserInput(params) => &params.request_id,
            Self::Elicitation(params) => &params.request_id,
            Self::McpRouteMessage { request_id, .. }
            | Self::HookCallback { request_id, .. }
            | Self::Error(ServerRequestErrorReply { request_id, .. }) => request_id,
        }
    }

    pub fn interactive_target(&self) -> Option<&InteractiveTarget> {
        match self {
            Self::Approval(params) => Some(&params.target),
            Self::UserInput(params) => Some(&params.target),
            Self::Elicitation(params) => Some(&params.target),
            Self::McpRouteMessage { .. } | Self::HookCallback { .. } | Self::Error(_) => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ServerRequestErrorReply {
    pub request_id: String,
    pub code: i32,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

#[stack_trace_debug]
#[derive(Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum AppServerError {
    #[snafu(display("{source}"))]
    Registry {
        source: RegistryError,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("calling surface is not attached: {surface_id}"))]
    CallingSurfaceNotAttached {
        surface_id: SurfaceId,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display(
        "calling surface {surface_id} belongs to session {actual_session_id}, expected {expected_session_id}"
    ))]
    CallingSurfaceWrongSession {
        surface_id: SurfaceId,
        expected_session_id: SessionId,
        actual_session_id: SessionId,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("calling surface belongs to another connection: {surface_id}"))]
    CallingSurfaceWrongConnection {
        surface_id: SurfaceId,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("calling surface is not an attached interactive surface: {surface_id}"))]
    CallingSurfaceNotInteractive {
        surface_id: SurfaceId,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("session already has an interactive owner: {session_id}"))]
    InteractiveOwnerConflict {
        session_id: SessionId,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("target session is not live ({state}): {session_id}"))]
    TargetSessionNotLive {
        session_id: SessionId,
        state: &'static str,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("server request was not found: {request_id:?}"))]
    ServerRequestNotFound {
        request_id: RequestId,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display(
        "server request {request_id:?} belongs to session {expected_session_id}, got {actual_session_id}"
    ))]
    ServerRequestWrongSession {
        request_id: RequestId,
        expected_session_id: SessionId,
        actual_session_id: SessionId,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display(
        "server request {request_id:?} belongs to surface {expected_surface_id}, got {actual_surface_id}"
    ))]
    ServerRequestWrongSurface {
        request_id: RequestId,
        expected_surface_id: SurfaceId,
        actual_surface_id: SurfaceId,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("server request belongs to another connection: {request_id:?}"))]
    ServerRequestWrongConnection {
        request_id: RequestId,
        #[snafu(implicit)]
        location: Location,
    },
}

impl ErrorExt for AppServerError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::Registry { source, .. } => source.status_code(),
            Self::CallingSurfaceNotAttached { .. }
            | Self::CallingSurfaceWrongSession { .. }
            | Self::CallingSurfaceWrongConnection { .. }
            | Self::CallingSurfaceNotInteractive { .. }
            | Self::InteractiveOwnerConflict { .. } => StatusCode::InvalidArguments,
            Self::TargetSessionNotLive { .. } => StatusCode::Cancelled,
            Self::ServerRequestNotFound { .. } => StatusCode::FileNotFound,
            Self::ServerRequestWrongSession { .. }
            | Self::ServerRequestWrongSurface { .. }
            | Self::ServerRequestWrongConnection { .. } => StatusCode::InvalidArguments,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl From<CompleteServerRequestError> for AppServerError {
    fn from(error: CompleteServerRequestError) -> Self {
        match error {
            CompleteServerRequestError::NotFound { request_id } => {
                ServerRequestNotFoundSnafu { request_id }.build()
            }
            CompleteServerRequestError::WrongSession {
                request_id,
                expected_session_id,
                actual_session_id,
            } => ServerRequestWrongSessionSnafu {
                request_id,
                expected_session_id,
                actual_session_id,
            }
            .build(),
            CompleteServerRequestError::WrongSurface {
                request_id,
                expected_surface_id,
                actual_surface_id,
            } => ServerRequestWrongSurfaceSnafu {
                request_id,
                expected_surface_id,
                actual_surface_id,
            }
            .build(),
            CompleteServerRequestError::WrongConnection { request_id } => {
                ServerRequestWrongConnectionSnafu { request_id }.build()
            }
        }
    }
}

#[cfg(test)]
#[path = "app_server.test.rs"]
mod tests;
