use std::{
    future::Future,
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

use coco_error::{ErrorExt, Location, StatusCode, stack_trace_debug};
use coco_types::{
    ApprovalResolveParams, ElicitationResolveParams, RequestId, ServerRequest, SessionAccess,
    SessionEnvelope, SessionId, SessionLifecycleEffect, SessionLifecycleEffectKind, SessionTarget,
    TurnId, UserInputResolveParams,
};
use snafu::{IntoError, ResultExt, Snafu};

use crate::{
    AttachError, AttachSessionOptions, CancelServerRequestOutcome, CloseCompletion,
    CloseSessionAttachmentsOutcome, CloseStart, CompleteServerRequestError, ConnectionCallback,
    ConnectionKey, ConnectionLimits, DetachSessionOutcome, DisconnectOutcome,
    LifecycleRouteOutcome, LiveSessionRegistry, LoadCompletion, LoadStart, OutboundSender,
    PendingServerRequest, ReplaceAttachmentError, ReplaceAttachmentOutcome, ReplaceStart,
    RouteOutcome, RoutingState, ServerRequestAudience, ServerRequestReplyKind,
    ServerRequestRouteError, ServerRequestSender, SessionActivityTracker, SessionAttachment,
    SessionConnectionCounts, SessionLifecycleSender, SubscribeReplay,
    TargetedSessionLifecycleEffect,
    registry::{CloseState, ClosingState, RegistryError, SessionSlot},
};

/// App-server state holder for registry + routing lock ordering.
///
/// It owns lifecycle owner tasks and the no-await commit sections that touch
/// both lifecycle slots and session routing. Runtime construction and close
/// behavior remain opaque futures supplied by the application host.
pub struct AppServer<H> {
    registry: LiveSessionRegistry<H>,
    routing: RwLock<RoutingState>,
    activity: SessionActivityTracker,
    server_request_waiters: std::sync::Mutex<
        std::collections::HashMap<RequestId, tokio::sync::oneshot::Sender<ServerRequestReply>>,
    >,
    server_request_timeout: Duration,
    /// Retained join handles for lifecycle owner tasks (load/close/replace) so
    /// process shutdown can abort and join any still in flight rather than
    /// detaching them (CS-3c).
    owner_tasks: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl<H: Clone> AppServer<H> {
    pub fn new(max_sessions: usize, retention_per_session: usize) -> Self {
        Self::with_connection_limits_and_server_request_timeout(
            max_sessions,
            retention_per_session,
            ConnectionLimits::default(),
            Duration::from_secs(15 * 60),
        )
    }

    pub fn with_server_request_timeout(
        max_sessions: usize,
        retention_per_session: usize,
        server_request_timeout: Duration,
    ) -> Self {
        Self::with_connection_limits_and_server_request_timeout(
            max_sessions,
            retention_per_session,
            ConnectionLimits::default(),
            server_request_timeout,
        )
    }

    pub fn with_connection_limits_and_server_request_timeout(
        max_sessions: usize,
        retention_per_session: usize,
        connection_limits: ConnectionLimits,
        server_request_timeout: Duration,
    ) -> Self {
        assert!(
            !server_request_timeout.is_zero(),
            "server request timeout must be non-zero"
        );
        Self {
            registry: LiveSessionRegistry::new(max_sessions),
            routing: RwLock::new(RoutingState::new_with_connection_limits(
                retention_per_session,
                connection_limits,
            )),
            activity: SessionActivityTracker::default(),
            server_request_waiters: std::sync::Mutex::new(std::collections::HashMap::new()),
            server_request_timeout,
            owner_tasks: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn registry(&self) -> &LiveSessionRegistry<H> {
        &self.registry
    }

    /// Resolve one explicitly targeted session while holding registry and
    /// routing locks in their canonical order.
    pub fn validate_session_target(
        &self,
        connection: ConnectionKey,
        target: &SessionTarget,
        required_access: SessionAccess,
    ) -> Result<ValidatedSession<H>, AppServerError> {
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
            .attachment(connection, &target.session_id)
            .ok_or_else(|| {
                SessionNotAttachedSnafu {
                    session_id: target.session_id.clone(),
                }
                .build()
            })?;
        let grant = routing
            .require_access(connection, &target.session_id, required_access)
            .map_err(AppServerError::from)?;
        let handle = match sessions.slots.get(&target.session_id) {
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
        Ok(ValidatedSession {
            handle,
            grant,
            attachment: attachment.clone(),
        })
    }

    /// Validate authorization without requiring a live runtime or event
    /// attachment. Durable reads and explicit deletion use this after close.
    pub fn validate_session_grant(
        &self,
        connection: ConnectionKey,
        target: &SessionTarget,
        required_access: SessionAccess,
    ) -> Result<crate::SessionGrant, AppServerError> {
        self.routing
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .require_access(connection, &target.session_id, required_access)
            .map_err(AppServerError::from)
    }

    pub fn validate_live_session(&self, session_id: &SessionId) -> Result<H, AppServerError> {
        let sessions = self
            .registry
            .sessions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match sessions.slots.get(session_id) {
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

    pub fn commit_replace_for_connection(
        &self,
        old_session_id: &SessionId,
        new_session_id: &SessionId,
        new_handle: H,
        connection: ConnectionKey,
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
        let old_handle = match sessions.slots.get(old_session_id) {
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
        if !matches!(
            sessions.slots.get(new_session_id),
            Some(SessionSlot::Loading(_))
        ) {
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
        if let Err(error) = routing.require_full(connection, old_session_id) {
            let error = AppServerError::from(error);
            return Err(ReplaceCommitFailure::new(error, new_handle));
        }

        let routing_outcome = match routing.replace_calling_attachment(
            connection,
            old_session_id,
            new_session_id.clone(),
        ) {
            Ok(outcome) => outcome,
            Err(ReplaceAttachmentError::Access(error)) => {
                return Err(ReplaceCommitFailure::new(
                    AppServerError::from(error),
                    new_handle,
                ));
            }
            Err(ReplaceAttachmentError::Attach(source)) => {
                return Err(ReplaceCommitFailure::new(
                    AttachSnafu.into_error(source),
                    new_handle,
                ));
            }
        };

        // Everything validated — mutate with no further fallible step.
        let old_close = CloseState::new();
        let old_close_completion = old_close.completion();
        // Consume the new reservation so a close-after-load recorded on it is
        // honored: the slot promotes to `Live`, or straight to `Closing`
        // if a shutdown raced the replace.
        let Some(SessionSlot::Loading(new_load)) = sessions.slots.remove(new_session_id) else {
            unreachable!("new slot was matched as Loading above");
        };
        sessions
            .slots
            .insert(new_session_id.clone(), new_load.promote(new_handle));
        sessions.slots.insert(
            old_session_id.clone(),
            SessionSlot::Closing(ClosingState {
                handle: old_handle.clone(),
                close: old_close,
            }),
        );

        let orphaned = routing.take_orphaned_waiter_cancellations();
        drop(routing);
        drop(sessions);
        self.cancel_server_request_waiters(&routing_outcome.cancelled_requests);
        self.cancel_server_request_waiters(&orphaned);
        let lifecycle_effects = replace_lifecycle_effects(&routing_outcome);
        self.activity.touch(new_session_id.clone());

        Ok(AppReplaceCommit {
            old_handle,
            old_close_completion,
            routing_outcome,
            lifecycle_effects,
        })
    }

    /// Atomically repoint a full-access connection to an already-live
    /// destination and move the source session into `Closing`.
    pub fn commit_replace_to_live_for_connection(
        &self,
        old_session_id: &SessionId,
        new_session_id: &SessionId,
        connection: ConnectionKey,
    ) -> Result<AppReplaceCommit<H>, AppServerError> {
        let mut sessions = self
            .registry
            .sessions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let old_handle = match sessions.slots.get(old_session_id) {
            Some(SessionSlot::Live(handle)) => handle.clone(),
            _ => {
                return crate::registry::OldNotReadySnafu {
                    session_id: old_session_id.clone(),
                }
                .fail()
                .context(RegistrySnafu);
            }
        };
        if !matches!(
            sessions.slots.get(new_session_id),
            Some(SessionSlot::Live(_))
        ) {
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
        routing
            .require_full(connection, old_session_id)
            .map_err(AppServerError::from)?;
        let routing_outcome = routing
            .replace_calling_attachment(connection, old_session_id, new_session_id.clone())
            .map_err(|error| match error {
                ReplaceAttachmentError::Access(error) => AppServerError::from(error),
                ReplaceAttachmentError::Attach(source) => AttachSnafu.into_error(source),
            })?;

        let old_close = CloseState::new();
        let old_close_completion = old_close.completion();
        sessions.slots.insert(
            old_session_id.clone(),
            SessionSlot::Closing(ClosingState {
                handle: old_handle.clone(),
                close: old_close,
            }),
        );
        let orphaned = routing.take_orphaned_waiter_cancellations();
        drop(routing);
        drop(sessions);
        self.cancel_server_request_waiters(&routing_outcome.cancelled_requests);
        self.cancel_server_request_waiters(&orphaned);
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
        let close_sender = match sessions.slots.get(session_id) {
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
        let routing_outcome = routing.close_session_attachments(session_id);
        let lifecycle_effects = close_lifecycle_effects(session_id, &routing_outcome);
        let _ = close_sender.send(Some(close_result));
        // Terminal removal: drop the slot plus its policy and, if this was a
        // sidechat child, its parent→child index entry.
        sessions.forget(session_id);
        let orphaned = routing.take_orphaned_waiter_cancellations();
        drop(routing);
        drop(sessions);
        self.cancel_server_request_waiters(&routing_outcome.cancelled_requests);
        self.cancel_server_request_waiters(&orphaned);
        self.activity.forget(session_id);

        Ok(AppCloseCommit {
            routing_outcome,
            lifecycle_effects,
        })
    }

    pub fn resolve_server_request(
        &self,
        connection: ConnectionKey,
        target: &SessionTarget,
        reply: ServerRequestReply,
    ) -> Result<ServerRequestResolution, AppServerError> {
        let request_id = RequestId::String(reply.request_id().to_string());
        self.resolve_server_request_for_connection(
            connection,
            &target.session_id,
            &request_id,
            reply,
        )
    }

    pub fn resolve_server_request_for_connection(
        &self,
        connection: ConnectionKey,
        session_id: &SessionId,
        request_id: &RequestId,
        reply: ServerRequestReply,
    ) -> Result<ServerRequestResolution, AppServerError> {
        let mut routing = self
            .routing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // An error reply is not a valid answer: it completes only a
        // connection-targeted request (whose sole responder failed) and merely
        // withdraws the sender from a broadcast — "first valid reply wins"
        // must not let one client's failure consume a prompt for its peers.
        let resolution = if reply.kind().is_none() {
            routing
                .resolve_error_reply(connection, session_id, request_id)
                .map(|disposition| match disposition {
                    crate::ErrorReplyDisposition::CompletedTargeted(pending) => {
                        ServerRequestResolution::Completed(ResolvedServerRequest {
                            pending,
                            reply: reply.clone(),
                        })
                    }
                    crate::ErrorReplyDisposition::Withdrawn => ServerRequestResolution::Withdrawn {
                        request_id: request_id.clone(),
                    },
                    crate::ErrorReplyDisposition::CancelledLast(pending) => {
                        ServerRequestResolution::Cancelled(pending)
                    }
                })
        } else {
            routing
                .complete_server_request(connection, session_id, request_id, reply.kind())
                .map(|pending| {
                    ServerRequestResolution::Completed(ResolvedServerRequest {
                        pending,
                        reply: reply.clone(),
                    })
                })
        };
        let orphaned = routing.take_orphaned_waiter_cancellations();
        drop(routing);
        self.cancel_server_request_waiters(&orphaned);
        let resolution = resolution.map_err(AppServerError::from)?;
        match &resolution {
            ServerRequestResolution::Completed(resolved) => {
                if let Some(waiter) = self
                    .server_request_waiters
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(request_id)
                {
                    let _ = waiter.send(resolved.reply.clone());
                }
            }
            ServerRequestResolution::Withdrawn { .. } => {}
            ServerRequestResolution::Cancelled(_) => {
                self.cancel_server_request_waiters(std::slice::from_ref(request_id));
            }
        }
        Ok(resolution)
    }

    pub fn cancel_server_request_for_connection(
        &self,
        connection: ConnectionKey,
        request_id: &RequestId,
    ) -> Result<CancelServerRequestOutcome, AppServerError> {
        let mut routing = self
            .routing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let outcome = routing.cancel_server_request_for_connection(request_id, connection);
        let orphaned = routing.take_orphaned_waiter_cancellations();
        drop(routing);
        self.cancel_server_request_waiters(&orphaned);
        let outcome = outcome.map_err(AppServerError::from)?;
        if matches!(outcome, CancelServerRequestOutcome::Cancelled(_)) {
            self.cancel_server_request_waiters(std::slice::from_ref(request_id));
        }
        Ok(outcome)
    }

    pub fn connect_with_request_and_lifecycle_senders(
        &self,
        connection: ConnectionKey,
        sender: OutboundSender,
        request_sender: ServerRequestSender,
        lifecycle_sender: SessionLifecycleSender,
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

    /// Attach only if the target session is live. The registry guard remains
    /// held across the routing mutation to prevent close/attach races.
    pub fn attach_live_session(
        &self,
        connection: ConnectionKey,
        session_id: SessionId,
        options: AttachSessionOptions,
    ) -> Result<(), AttachError> {
        let registry = self
            .registry
            .sessions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::ensure_live_slot(&registry, &session_id)?;
        let mut routing = self
            .routing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let attached = routing.attach_session(connection, session_id.clone(), options);
        let orphaned = routing.take_orphaned_waiter_cancellations();
        drop(routing);
        drop(registry);
        self.cancel_server_request_waiters(&orphaned);
        attached?;
        self.activity.touch(session_id);
        Ok(())
    }

    /// Registry-slot guard shared by the live attach/subscribe paths.
    fn ensure_live_slot(
        inner: &crate::registry::RegistryInner<H>,
        session_id: &SessionId,
    ) -> Result<(), AttachError> {
        match inner.slots.get(session_id) {
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

    /// Read-only subscribe only if the target session is live.
    pub fn subscribe_live_session(
        &self,
        connection: ConnectionKey,
        session_id: SessionId,
        after_seq: Option<i64>,
        options: AttachSessionOptions,
    ) -> Result<SubscribeReplay, AttachError> {
        let registry = self
            .registry
            .sessions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::ensure_live_slot(&registry, &session_id)?;
        let mut routing = self
            .routing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let result = routing.subscribe(connection, session_id.clone(), after_seq, options);
        let orphaned = routing.take_orphaned_waiter_cancellations();
        drop(routing);
        drop(registry);
        self.cancel_server_request_waiters(&orphaned);
        let result = result?;
        self.activity.touch(session_id);
        Ok(result)
    }

    pub fn route_envelope(&self, envelope: SessionEnvelope) -> RouteOutcome {
        let session_id = envelope.session_id.clone();
        let mut routing = self
            .routing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let outcome = routing.route_envelope(envelope);
        let orphaned = routing.take_orphaned_waiter_cancellations();
        drop(routing);
        // A slow-consumer disconnect inside routing removes that connection's
        // targeted pending requests; resolve their reply waiters here so a
        // hook/MCP bridge await fails fast instead of stranding forever.
        self.cancel_server_request_waiters(&orphaned);
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
        let orphaned = routing.take_orphaned_waiter_cancellations();
        drop(routing);
        self.cancel_server_request_waiters(&outcome.cancelled_requests);
        self.cancel_server_request_waiters(&orphaned);
        for session_id in session_ids {
            self.activity.touch(session_id);
        }
        outcome
    }

    pub fn detach_session_for_connection(
        &self,
        connection: ConnectionKey,
        session_id: &SessionId,
    ) -> DetachSessionOutcome {
        let mut routing = self
            .routing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let outcome = routing.detach_session_for_connection(connection, session_id);
        let orphaned = routing.take_orphaned_waiter_cancellations();
        drop(routing);
        self.cancel_server_request_waiters(&outcome.cancelled_requests);
        self.cancel_server_request_waiters(&orphaned);
        if outcome.detached {
            self.activity.touch(session_id.clone());
        }
        outcome
    }

    pub fn revoke_session_grants(&self, session_id: &SessionId) {
        self.routing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .revoke_session_grants(session_id);
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
            .slots
            .iter()
            .filter(|&(_, slot)| matches!(slot, SessionSlot::Live(_)))
            // Internal slots (sidechat children) never appear in the public
            // `session/list` catalog.
            .filter(|&(id, _)| {
                sessions
                    .policies
                    .get(id)
                    .is_none_or(|policy| !policy.is_internal())
            })
            .map(|(session_id, _)| AppLiveSessionSummary {
                session_id: session_id.clone(),
                connection_counts: routing.connection_counts_for_session(session_id),
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
            .slots
            .contains_key(session_id)
    }

    pub fn register_connection_callback(
        &self,
        connection: ConnectionKey,
        session_id: SessionId,
        callback: ConnectionCallback,
    ) -> Result<(), crate::SessionAccessError> {
        self.routing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .register_connection_callback(connection, session_id, callback)
    }

    pub fn connection_callback_owner(
        &self,
        session_id: &SessionId,
        callback: &ConnectionCallback,
    ) -> Option<ConnectionKey> {
        self.routing
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .connection_callback_owner(session_id, callback)
    }

    /// Session ids that must remain announced to process egress: Live plus
    /// retiring Closing slots whose final `SessionResult` may still be in
    /// flight. Used by Event Hub membership so a closing session is not dropped
    /// before its final local-egress handoff completes (CS-4 / R17).
    pub fn announced_session_ids(&self) -> Vec<SessionId> {
        self.registry.list_announced()
    }

    pub fn route_server_request_with_reply(
        self: &Arc<Self>,
        session_id: SessionId,
        turn_id: Option<TurnId>,
        request: ServerRequest,
    ) -> Result<tokio::sync::oneshot::Receiver<ServerRequestReply>, ServerRequestRouteError>
    where
        H: Send + Sync + 'static,
    {
        self.route_server_request_with_reply_to(
            ServerRequestAudience::AllFullConnections,
            session_id,
            turn_id,
            request,
        )
    }

    pub fn route_server_request_with_reply_to_connection(
        self: &Arc<Self>,
        connection: ConnectionKey,
        session_id: SessionId,
        turn_id: Option<TurnId>,
        request: ServerRequest,
    ) -> Result<tokio::sync::oneshot::Receiver<ServerRequestReply>, ServerRequestRouteError>
    where
        H: Send + Sync + 'static,
    {
        self.route_server_request_with_reply_to(
            ServerRequestAudience::Connection(connection),
            session_id,
            turn_id,
            request,
        )
    }

    fn route_server_request_with_reply_to(
        self: &Arc<Self>,
        audience: ServerRequestAudience,
        session_id: SessionId,
        turn_id: Option<TurnId>,
        request: ServerRequest,
    ) -> Result<tokio::sync::oneshot::Receiver<ServerRequestReply>, ServerRequestRouteError>
    where
        H: Send + Sync + 'static,
    {
        // Lock-order note: this function deliberately holds the routing write
        // lock across the waiter insert and the publish. That continuous hold
        // is what guarantees waiter-before-publish (an immediate client reply
        // can never beat waiter registration) and keeps the prepared entry
        // alive for `publish_prepared_server_request`. The nesting is safe
        // because no code path acquires the routing lock while holding the
        // waiters mutex.
        let mut routing = self
            .routing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pending = match routing.prepare_server_request(audience, session_id, turn_id, request) {
            Ok(pending) => pending,
            Err(error) => {
                let orphaned = routing.take_orphaned_waiter_cancellations();
                drop(routing);
                self.cancel_server_request_waiters(&orphaned);
                return Err(error);
            }
        };
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.server_request_waiters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(pending.request_id.clone(), sender);
        let published = routing.publish_prepared_server_request(&pending.request_id);
        let orphaned = routing.take_orphaned_waiter_cancellations();
        drop(routing);
        self.cancel_server_request_waiters(&orphaned);
        match published {
            Ok(_) => {
                let weak_server = Arc::downgrade(self);
                let request_id = pending.request_id;
                let timeout = self.server_request_timeout;
                tokio::spawn(async move {
                    tokio::time::sleep(timeout).await;
                    if let Some(server) = weak_server.upgrade() {
                        server.expire_server_request(&request_id);
                    }
                });
                Ok(receiver)
            }
            Err(error) => {
                self.cancel_server_request_waiters(std::slice::from_ref(&pending.request_id));
                Err(error)
            }
        }
    }

    /// Cancel every pending server->client request scoped to `turn_id`, removing
    /// its routing bookkeeping and resolving its waiter. Called when a turn ends
    /// so an interrupted turn's outstanding approval/hook/MCP requests do not
    /// leak their pending entries + retained payloads until the session closes.
    pub fn cancel_turn_server_requests(&self, turn_id: &TurnId) {
        let mut routing = self
            .routing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let cancelled = routing.cancel_turn_server_requests(turn_id);
        let orphaned = routing.take_orphaned_waiter_cancellations();
        drop(routing);
        self.cancel_server_request_waiters(&cancelled);
        self.cancel_server_request_waiters(&orphaned);
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

    fn expire_server_request(&self, request_id: &RequestId) {
        let mut routing = self
            .routing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let removed = routing.expire_server_request(request_id);
        let orphaned = routing.take_orphaned_waiter_cancellations();
        drop(routing);
        self.cancel_server_request_waiters(&orphaned);
        if removed {
            self.cancel_server_request_waiters(std::slice::from_ref(request_id));
        }
    }

    #[cfg(test)]
    pub(crate) fn pending_server_request_replays_for_session(
        &self,
        session_id: &SessionId,
    ) -> Vec<crate::PendingServerRequestReplay> {
        self.routing
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending_server_request_replays_for_session(session_id)
    }

    pub fn route_lifecycle_effects(
        &self,
        effects: Vec<TargetedSessionLifecycleEffect>,
    ) -> LifecycleRouteOutcome {
        let mut routing = self
            .routing
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let outcome = routing.route_lifecycle_effects(effects);
        let orphaned = routing.take_orphaned_waiter_cancellations();
        drop(routing);
        self.cancel_server_request_waiters(&orphaned);
        outcome
    }
}

#[derive(Debug, Clone)]
pub struct ValidatedSession<H> {
    pub handle: H,
    pub grant: crate::SessionGrant,
    pub attachment: SessionAttachment,
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

    pub fn spawn_load<F, Close, CloseFut>(
        self: &Arc<Self>,
        session_id: SessionId,
        factory: F,
        // Tears a constructed runtime down when the load commit fails (e.g. a
        // close raced the promotion). Never dropped silently — the same
        // rule as `spawn_replace`'s commit-failure path.
        teardown: Close,
    ) -> Result<AppLoadStart<H>, AppServerError>
    where
        F: Future<Output = Result<H, RegistryError>> + Send + 'static,
        Close: FnOnce(H) -> CloseFut + Send + 'static,
        CloseFut: Future<Output = Result<(), RegistryError>> + Send + 'static,
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
                            match server.registry.complete_load_success(&session_id, handle) {
                                Ok(()) => server.activity.touch(session_id.clone()),
                                Err(failure) => {
                                    tracing::warn!(
                                        error = %failure.error,
                                        session_id = %session_id,
                                        "load commit failed; tearing down the constructed runtime"
                                    );
                                    let _ = teardown(failure.handle).await;
                                    let _ = server
                                        .registry
                                        .complete_load_failure(&session_id, failure.error);
                                }
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

    /// Reserve and load a sidechat child of a live `parent`, enforcing the
    /// one-child-per-parent invariant atomically via `begin_child_load` (the
    /// child slot is stamped `Child/Internal/LocalOnly`). Otherwise identical to
    /// [`Self::spawn_load`].
    pub fn spawn_child_load<F, Close, CloseFut>(
        self: &Arc<Self>,
        parent: SessionId,
        child: SessionId,
        factory: F,
        // See `spawn_load`: runs when the child construction finishes but the
        // commit fails (parent closed/blocked mid-construction without a
        // recorded close-after-load).
        teardown: Close,
    ) -> Result<AppLoadStart<H>, AppServerError>
    where
        F: Future<Output = Result<H, RegistryError>> + Send + 'static,
        Close: FnOnce(H) -> CloseFut + Send + 'static,
        CloseFut: Future<Output = Result<(), RegistryError>> + Send + 'static,
    {
        match self
            .registry
            .begin_child_load(&parent, child.clone())
            .context(RegistrySnafu)?
        {
            LoadStart::Reserved => {
                let LoadStart::Loading(completion) = self
                    .registry
                    .begin_load(child.clone())
                    .context(RegistrySnafu)?
                else {
                    unreachable!("reserved child load must be observable as Loading");
                };
                let server = Arc::clone(self);
                self.spawn_tracked(async move {
                    let mut guard = OwnerGuard::new(
                        Arc::clone(&server),
                        OwnerGuardAction::FailLoad(child.clone()),
                    );
                    match factory.await {
                        Ok(handle) => match server.registry.complete_load_success(&child, handle) {
                            Ok(()) => server.activity.touch(child.clone()),
                            Err(failure) => {
                                tracing::warn!(
                                    error = %failure.error,
                                    session_id = %child,
                                    "child load commit failed; tearing down the constructed runtime"
                                );
                                let _ = teardown(failure.handle).await;
                                let _ =
                                    server.registry.complete_load_failure(&child, failure.error);
                            }
                        },
                        Err(error) => {
                            let _ = server.registry.complete_load_failure(&child, error);
                        }
                    }
                    guard.disarm();
                });
                Ok(AppLoadStart::Started { completion })
            }
            LoadStart::Live(handle) => {
                self.activity.touch(child);
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
        C: Fn(H) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = Result<(), RegistryError>> + Send + 'static,
    {
        let cascade = self
            .registry
            .begin_close_cascade(&session_id)
            .context(RegistrySnafu)?;
        let child_completion = cascade.child.map(|(child_id, child_start)| {
            self.spawn_close_from_start(child_id, child_start, close.clone(), None)
                .completion()
        });
        Ok(self.spawn_close_from_start(session_id, cascade.parent, close, child_completion))
    }

    /// [`Self::spawn_close`] that commits the `Live -> Closing` transition
    /// only while no connection is attached to the session. The check runs
    /// under the registry write lock, and live attaches hold the registry
    /// read lock across their routing mutation, so an attach can never land
    /// between the check and the transition — the idle supervisor's
    /// check-then-close race is structurally closed. Aborts with
    /// `RegistryError::CloseAborted`.
    pub fn spawn_close_when_unattached<C, Fut>(
        self: &Arc<Self>,
        session_id: SessionId,
        close: C,
    ) -> Result<AppCloseStart, AppServerError>
    where
        C: Fn(H) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = Result<(), RegistryError>> + Send + 'static,
    {
        let cascade = self
            .registry
            .begin_close_cascade_if(&session_id, || {
                self.routing
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .connection_counts_for_session(&session_id)
                    .total()
                    == 0
            })
            .context(RegistrySnafu)?;
        let child_completion = cascade.child.map(|(child_id, child_start)| {
            self.spawn_close_from_start(child_id, child_start, close.clone(), None)
                .completion()
        });
        Ok(self.spawn_close_from_start(session_id, cascade.parent, close, child_completion))
    }

    fn spawn_close_from_start<C, Fut>(
        self: &Arc<Self>,
        session_id: SessionId,
        start: CloseStart<H>,
        close: C,
        mut prerequisite: Option<CloseCompletion>,
    ) -> AppCloseStart
    where
        C: Fn(H) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = Result<(), RegistryError>> + Send + 'static,
    {
        match start {
            CloseStart::Started { handle, completion } => {
                let server = Arc::clone(self);
                self.spawn_tracked(async move {
                    let mut guard = OwnerGuard::new(
                        Arc::clone(&server),
                        OwnerGuardAction::Close(session_id.clone()),
                    );
                    if let Some(prerequisite) = prerequisite.as_mut() {
                        let _ = prerequisite.wait().await;
                    }
                    let close_result = close(handle).await;
                    if let Ok(commit) = server.complete_session_close(&session_id, close_result) {
                        server.route_lifecycle_effects(commit.lifecycle_effects);
                    }
                    guard.disarm();
                });
                AppCloseStart::Started { completion }
            }
            CloseStart::Loading {
                mut load_completion,
                close_completion,
                should_spawn,
            } => {
                if should_spawn {
                    let server = Arc::clone(self);
                    let close_session_id = session_id;
                    self.spawn_tracked(async move {
                        // Not guarded during the load wait: a load failure fires
                        // the close signal and removes the slot (there is nothing
                        // to close). Guard only the actual cascade below.
                        if let Ok(handle) = load_completion.wait().await {
                            let mut guard = OwnerGuard::new(
                                Arc::clone(&server),
                                OwnerGuardAction::Close(close_session_id.clone()),
                            );
                            if let Some(prerequisite) = prerequisite.as_mut() {
                                let _ = prerequisite.wait().await;
                            }
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
                AppCloseStart::Loading(close_completion)
            }
            CloseStart::Closing { completion, .. } => AppCloseStart::Closing(completion),
        }
    }

    pub fn spawn_replace<F, Close, CloseFut>(
        self: &Arc<Self>,
        old_session_id: SessionId,
        new_session_id: SessionId,
        connection: ConnectionKey,
        factory: F,
        // Runs the close cascade for a handle, deriving its target from the
        // handle itself. Invoked on the OLD handle after a successful commit,
        // or on the NEW handle to tear it down when the commit fails.
        close_handle: Close,
    ) -> Result<AppReplaceStart<H>, AppServerError>
    where
        F: Future<Output = Result<H, RegistryError>> + Send + 'static,
        Close: Fn(H) -> CloseFut + Clone + Send + Sync + 'static,
        CloseFut: Future<Output = Result<(), RegistryError>> + Send + 'static,
    {
        let ReplaceStart::Reserved {
            new_completion,
            child,
            ..
        } = self
            .registry
            .begin_replace(&old_session_id, new_session_id.clone())
            .context(RegistrySnafu)?;
        let child_completion = child.map(|(child_id, child_start)| {
            self.spawn_close_from_start(child_id, child_start, close_handle.clone(), None)
                .completion()
        });
        let server = Arc::clone(self);
        self.spawn_tracked(async move {
            let mut guard = OwnerGuard::new(
                Arc::clone(&server),
                OwnerGuardAction::FailLoad(new_session_id.clone()),
            );
            match factory.await {
                Ok(new_handle) => {
                    if let Some(mut child_completion) = child_completion {
                        let _ = child_completion.wait().await;
                    }
                    match server.commit_replace_for_connection(
                        &old_session_id,
                        &new_session_id,
                        new_handle,
                        connection,
                    ) {
                        Ok(commit) => {
                            // Committed: new is live, old is Closing. The hazard
                            // now is a panic in the old close cascade wedging old.
                            guard.arm_close(old_session_id.clone());
                            server.route_lifecycle_effects(commit.lifecycle_effects);
                            let close_result = close_handle.clone()(commit.old_handle).await;
                            if let Ok(close_commit) =
                                server.complete_session_close(&old_session_id, close_result)
                            {
                                server.route_lifecycle_effects(close_commit.lifecycle_effects);
                            }
                            guard.disarm();
                        }
                        Err(failure) => {
                            // Commit failed after the factory built a full
                            // runtime (e.g. the calling connection disconnected
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
                            let _ = close_handle.clone()(failure.handle).await;
                            let _ = server.registry.complete_replace_failure(
                                &old_session_id,
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
                    let _ = server.registry.complete_replace_failure(
                        &old_session_id,
                        &new_session_id,
                        error,
                    );
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
    /// Repoints the calling full-access connection from `old_session_id` to the
    /// already-live `new_session_id`, moves the source into `Closing`, then runs
    /// the supplied source close cascade in a tracked owner task under an
    /// `OwnerGuard`. This is the connection-aware sibling of `spawn_replace` for the
    /// case where the destination already exists. Hosts must route through this
    /// rather than hand-rolling the source close in a bare `tokio::spawn`: a
    /// panic there would wedge the source in `Closing` forever (every close
    /// waiter hangs, and the slot permanently consumes a `max_sessions` unit),
    /// and a bare spawn is not tracked for shutdown joining.
    pub fn spawn_replace_to_live<Close, CloseFut>(
        self: &Arc<Self>,
        old_session_id: SessionId,
        new_session_id: SessionId,
        connection: ConnectionKey,
        close_old: Close,
    ) -> Result<CloseCompletion, AppServerError>
    where
        Close: Fn(H) -> CloseFut + Clone + Send + Sync + 'static,
        CloseFut: Future<Output = Result<(), RegistryError>> + Send + 'static,
    {
        let child = self
            .registry
            .begin_parent_transition(&old_session_id)
            .context(RegistrySnafu)?;
        let child_completion = child.map(|(child_id, child_start)| {
            self.spawn_close_from_start(child_id, child_start, close_old.clone(), None)
                .completion()
        });
        let commit = match self.commit_replace_to_live_for_connection(
            &old_session_id,
            &new_session_id,
            connection,
        ) {
            Ok(commit) => commit,
            Err(error) => {
                self.registry.unblock_child_admission(&old_session_id);
                return Err(error);
            }
        };
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
            if let Some(mut child_completion) = child_completion {
                let _ = child_completion.wait().await;
            }
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

fn replace_lifecycle_effects(
    outcome: &ReplaceAttachmentOutcome,
) -> Vec<TargetedSessionLifecycleEffect> {
    let mut effects = Vec::with_capacity(outcome.detached_connections.len());
    effects.push(TargetedSessionLifecycleEffect {
        connection: outcome.calling_connection,
        effect: SessionLifecycleEffect {
            kind: SessionLifecycleEffectKind::SessionReplaced {
                old_session_id: outcome.old_session_id.clone(),
                new_session_id: outcome.new_session_id.clone(),
            },
        },
    });
    effects.extend(
        outcome
            .detached_connections
            .iter()
            .copied()
            .filter(|connection| *connection != outcome.calling_connection)
            .map(|connection| TargetedSessionLifecycleEffect {
                connection,
                effect: SessionLifecycleEffect {
                    kind: SessionLifecycleEffectKind::SessionEnded {
                        session_id: outcome.old_session_id.clone(),
                    },
                },
            }),
    );
    effects
}

fn close_lifecycle_effects(
    session_id: &SessionId,
    outcome: &CloseSessionAttachmentsOutcome,
) -> Vec<TargetedSessionLifecycleEffect> {
    outcome
        .detached_connections
        .iter()
        .copied()
        .map(|connection| TargetedSessionLifecycleEffect {
            connection,
            effect: SessionLifecycleEffect {
                kind: SessionLifecycleEffectKind::SessionEnded {
                    session_id: session_id.clone(),
                },
            },
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct AppReplaceCommit<H> {
    pub old_handle: H,
    pub old_close_completion: CloseCompletion,
    pub routing_outcome: ReplaceAttachmentOutcome,
    pub lifecycle_effects: Vec<TargetedSessionLifecycleEffect>,
}

/// Failure of `commit_replace_for_connection` that hands the un-committed new
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
    pub routing_outcome: CloseSessionAttachmentsOutcome,
    pub lifecycle_effects: Vec<TargetedSessionLifecycleEffect>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppLiveSessionSummary {
    pub session_id: SessionId,
    pub connection_counts: SessionConnectionCounts,
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

/// Outcome of one connection replying to a pending server request.
///
/// Valid typed replies always complete. An error reply completes only a
/// connection-targeted request; on a broadcast it withdraws the sender
/// (`Withdrawn`) or, when the sender was the last recipient, cancels the
/// request outright (`Cancelled`) — the reply waiter observes a closed
/// channel, exactly like an explicit last-recipient cancellation.
#[derive(Debug, Clone)]
pub enum ServerRequestResolution {
    Completed(ResolvedServerRequest),
    Withdrawn { request_id: RequestId },
    Cancelled(PendingServerRequest),
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

    pub fn session_target(&self) -> Option<&SessionTarget> {
        match self {
            Self::Approval(params) => Some(&params.target),
            Self::UserInput(params) => Some(&params.target),
            Self::Elicitation(params) => Some(&params.target),
            Self::McpRouteMessage { .. } | Self::HookCallback { .. } | Self::Error(_) => None,
        }
    }

    pub fn kind(&self) -> Option<ServerRequestReplyKind> {
        match self {
            Self::Approval(_) => Some(ServerRequestReplyKind::Approval),
            Self::UserInput(_) => Some(ServerRequestReplyKind::UserInput),
            Self::Elicitation(_) => Some(ServerRequestReplyKind::Elicitation),
            Self::McpRouteMessage { .. } => Some(ServerRequestReplyKind::McpRouteMessage),
            Self::HookCallback { .. } => Some(ServerRequestReplyKind::HookCallback),
            Self::Error(_) => None,
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
    #[snafu(display("{source}"))]
    Attach {
        source: AttachError,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("connection is not attached to session {session_id}"))]
    SessionNotAttached {
        session_id: SessionId,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("connection has no grant for session {session_id}"))]
    SessionGrantMissing {
        session_id: SessionId,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("session {session_id} grant is read-only"))]
    SessionGrantReadOnly {
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
    #[snafu(display("connection was not sent server request {request_id:?}"))]
    ServerRequestNotRecipient {
        request_id: RequestId,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display(
        "server request {request_id:?} expected a {expected:?} reply, got {actual:?}"
    ))]
    ServerRequestWrongReplyKind {
        request_id: RequestId,
        expected: ServerRequestReplyKind,
        actual: ServerRequestReplyKind,
        #[snafu(implicit)]
        location: Location,
    },
}

impl ErrorExt for AppServerError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::Registry { source, .. } => source.status_code(),
            Self::Attach { source, .. } => source.status_code(),
            Self::SessionNotAttached { .. }
            | Self::SessionGrantMissing { .. }
            | Self::SessionGrantReadOnly { .. } => StatusCode::InvalidArguments,
            Self::TargetSessionNotLive { .. } => StatusCode::Cancelled,
            Self::ServerRequestNotFound { .. } => StatusCode::FileNotFound,
            Self::ServerRequestWrongSession { .. }
            | Self::ServerRequestNotRecipient { .. }
            | Self::ServerRequestWrongReplyKind { .. } => StatusCode::InvalidArguments,
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
            CompleteServerRequestError::Access(source) => AppServerError::from(source),
            CompleteServerRequestError::NotRecipient { request_id, .. } => {
                ServerRequestNotRecipientSnafu { request_id }.build()
            }
            CompleteServerRequestError::WrongReplyKind {
                request_id,
                expected,
                actual,
            } => ServerRequestWrongReplyKindSnafu {
                request_id,
                expected,
                actual,
            }
            .build(),
        }
    }
}

impl From<crate::SessionAccessError> for AppServerError {
    fn from(error: crate::SessionAccessError) -> Self {
        match error {
            crate::SessionAccessError::MissingGrant { session_id, .. } => {
                SessionGrantMissingSnafu { session_id }.build()
            }
            crate::SessionAccessError::NotAttached { session_id, .. } => {
                SessionNotAttachedSnafu { session_id }.build()
            }
            crate::SessionAccessError::ReadOnly { session_id, .. } => {
                SessionGrantReadOnlySnafu { session_id }.build()
            }
        }
    }
}

#[cfg(test)]
#[path = "app_server.test.rs"]
mod tests;
