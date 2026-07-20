use super::*;

impl<H: Clone + Send + Sync + 'static> LocalServerClient<H> {
    pub fn attach_full_session(
        &self,
        session_id: SessionId,
    ) -> Result<LocalSessionClient, ClientError> {
        let attached = self
            .handle
            .attach_session(session_id, AttachSessionOptions::full())
            .map_err(|error| ClientError::InvalidArgument(error.to_string()))?;
        Ok(LocalSessionClient {
            session_id: attached.session_id,
        })
    }

    pub fn subscribe_session(
        &self,
        session_id: SessionId,
        after_seq: Option<i64>,
    ) -> Result<LocalReadOnlySessionClient, ClientError> {
        match self.handle.subscribe_session(
            session_id,
            after_seq,
            AttachSessionOptions::read_only(),
        ) {
            Ok(LocalClientSubscribeOutcome::Attached(subscription)) => {
                Ok(LocalReadOnlySessionClient {
                    session_id: subscription.session_id,
                    replayed: subscription.replayed,
                })
            }
            Ok(LocalClientSubscribeOutcome::SnapshotRequired) => Err(ClientError::SnapshotRequired),
            Err(error) => Err(ClientError::InvalidArgument(error.to_string())),
        }
    }

    pub fn detach_session(
        &self,
        session_id: &SessionId,
    ) -> Result<DetachSessionOutcome, ClientError> {
        let outcome = self.handle.detach_session(session_id);
        if outcome.detached {
            Ok(outcome)
        } else {
            Err(ClientError::InvalidArgument(format!(
                "connection is not attached to session {session_id}"
            )))
        }
    }

    pub fn list_live_sessions(&self) -> Vec<LocalLiveSessionSummary> {
        self.handle
            .list_live_sessions()
            .into_iter()
            .map(|summary| LocalLiveSessionSummary {
                session_id: summary.session_id,
                connection_counts: summary.connection_counts,
            })
            .collect()
    }
}

impl<H: Clone + Send + Sync + 'static> LocalServerClient<H> {
    /// Create an in-process observation handle without registering another
    /// AppServer connection or changing the connection's session grant.
    pub fn observe_session(&mut self, session_id: SessionId) -> LocalSessionClient {
        LocalSessionClient { session_id }
    }

    pub async fn next_session_event(
        &mut self,
        session: &LocalSessionClient,
    ) -> Option<SessionEnvelope> {
        loop {
            let notified = self.inbound_owner.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self
                .inbound_owner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .closed
            {
                return None;
            }
            tokio::select! {
                event = self.events.recv() => match event {
                    Ok(event) if &event.session_id == session.session_id() => return Some(event),
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                },
                () = &mut notified => {}
            }
        }
    }

    /// Wait for the next server request for `session`, preserving requests for
    /// other sessions. Run event and request dispatchers on separate cheap
    /// client clones; both remain in-memory views of the same connection.
    pub async fn next_session_request(
        &mut self,
        session: &LocalSessionClient,
    ) -> Option<coco_types::ServerRequestDelivery> {
        loop {
            let notified = self.inbound_owner.notify.notified();
            {
                let mut state = self
                    .inbound_owner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(request) = pop_session_request(&mut state, session.session_id()) {
                    return Some(request);
                }
                if state.closed {
                    return None;
                }
            }
            notified.await;
        }
    }

    /// Wait for the next server request on this connection, regardless of
    /// session. This is useful for clients that run one request dispatcher for
    /// every full-access session carried by the connection.
    pub async fn next_server_request(&mut self) -> Option<coco_types::ServerRequestDelivery> {
        loop {
            let notified = self.inbound_owner.notify.notified();
            {
                let mut state = self
                    .inbound_owner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(request) = pop_any_request(&mut state) {
                    return Some(request);
                }
                if state.closed {
                    return None;
                }
            }
            notified.await;
        }
    }

    pub fn try_next_session_request(
        &mut self,
        session: &LocalSessionClient,
    ) -> Option<coco_types::ServerRequestDelivery> {
        let mut state = self
            .inbound_owner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pop_session_request(&mut state, session.session_id())
    }

    pub fn try_next_session_event(
        &mut self,
        session: &LocalSessionClient,
    ) -> Option<SessionEnvelope> {
        loop {
            match self.events.try_recv() {
                Ok(event) if &event.session_id == session.session_id() => return Some(event),
                Ok(_) | Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
                | Err(tokio::sync::broadcast::error::TryRecvError::Closed) => return None,
            }
        }
    }

    pub(crate) async fn next_lifecycle_effect(
        &mut self,
    ) -> Option<coco_types::SessionLifecycleEffect> {
        loop {
            let notified = self.inbound_owner.notify.notified();
            {
                let mut state = self
                    .inbound_owner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(effect) = state.lifecycle.pop_front() {
                    return Some(effect);
                }
                if state.closed {
                    return None;
                }
            }
            notified.await;
        }
    }
}

fn pop_any_request(state: &mut LocalInboundState) -> Option<coco_types::ServerRequestDelivery> {
    let session_id = state
        .requests
        .iter()
        .find_map(|(session_id, queue)| (!queue.is_empty()).then(|| session_id.clone()))?;
    pop_session_request(state, &session_id)
}

fn pop_session_request(
    state: &mut LocalInboundState,
    session_id: &SessionId,
) -> Option<coco_types::ServerRequestDelivery> {
    let (request, empty) = {
        let queue = state.requests.get_mut(session_id)?;
        (queue.pop_front(), queue.is_empty())
    };
    if request.is_some() {
        state.request_count -= 1;
    }
    if empty {
        state.requests.remove(session_id);
    }
    request
}
