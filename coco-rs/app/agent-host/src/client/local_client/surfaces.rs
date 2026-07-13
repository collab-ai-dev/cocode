use super::*;

impl<H: Clone> LocalServerClient<H> {
    pub fn attach_interactive_session(
        &self,
        session_id: SessionId,
        mut options: AttachSurfaceOptions,
    ) -> Result<LocalSessionClient, ClientError> {
        options.role = SurfaceRole::Interactive;
        let surface = self
            .connection
            .attach_surface(session_id, options)
            .map_err(client_error_from_attach)?;
        Ok(LocalSessionClient {
            session_id: surface.session_id,
            surface_id: surface.surface_id,
        })
    }

    /// Live-only passive attach WITHOUT replay: attaches a passive surface via
    /// `attach_surface` (not `subscribe` with a cursor), so it never returns
    /// `SnapshotRequired`. This is the no-replay tail attach the TUI
    /// turn-completion monitors use instead of `subscribe_session (id, Some (0))`
    ///. The returned handle carries no replayed envelopes.
    pub fn attach_passive_session(
        &self,
        session_id: SessionId,
    ) -> Result<LocalPassiveSessionClient, ClientError> {
        let options = AttachSurfaceOptions {
            role: SurfaceRole::Passive,
            ..AttachSurfaceOptions::default()
        };
        let surface = self
            .connection
            .attach_surface(session_id, options)
            .map_err(client_error_from_attach)?;
        Ok(LocalPassiveSessionClient {
            session_id: surface.session_id,
            surface_id: surface.surface_id,
            replayed: Vec::new(),
        })
    }

    pub fn subscribe_session(
        &self,
        session_id: SessionId,
        after_seq: Option<i64>,
        options: AttachSurfaceOptions,
    ) -> Result<LocalPassiveSessionClient, ClientError> {
        let subscription = self
            .connection
            .subscribe_surface(session_id, after_seq, options)
            .map_err(client_error_from_attach)?;
        match subscription {
            LocalClientSubscribeOutcome::Attached(subscription) => Ok(LocalPassiveSessionClient {
                session_id: subscription.session_id,
                surface_id: subscription.surface_id,
                replayed: subscription.replayed,
            }),
            LocalClientSubscribeOutcome::SnapshotRequired => Err(ClientError::SnapshotRequired),
        }
    }

    pub fn events_mut(&mut self) -> &mut tokio::sync::mpsc::Receiver<SurfaceDelivery> {
        self.connection.events_mut()
    }

    pub fn try_next_session_event(
        &mut self,
        session: &LocalSessionClient,
    ) -> Option<SessionEnvelope> {
        self.try_next_event_for_surface(session.surface_id())
    }

    pub async fn next_session_event(
        &mut self,
        session: &LocalSessionClient,
    ) -> Option<SessionEnvelope> {
        self.next_event_for_surface(session.surface_id()).await
    }

    pub fn try_next_passive_event(
        &mut self,
        session: &LocalPassiveSessionClient,
    ) -> Option<SessionEnvelope> {
        self.try_next_event_for_surface(session.surface_id())
    }

    pub async fn next_passive_event(
        &mut self,
        session: &LocalPassiveSessionClient,
    ) -> Option<SessionEnvelope> {
        self.next_event_for_surface(session.surface_id()).await
    }

    pub fn server_requests_mut(
        &mut self,
    ) -> &mut tokio::sync::mpsc::Receiver<ServerRequestDelivery> {
        self.connection.server_requests_mut()
    }

    pub fn take_server_requests(&mut self) -> tokio::sync::mpsc::Receiver<ServerRequestDelivery> {
        self.connection.take_server_requests()
    }

    pub fn try_next_session_request(
        &mut self,
        session: &LocalSessionClient,
    ) -> Option<ServerRequestDelivery> {
        self.try_next_request_for_surface(session.surface_id())
    }

    pub fn lifecycle_mut(&mut self) -> &mut tokio::sync::mpsc::Receiver<SurfaceLifecycleEffect> {
        self.connection.lifecycle_mut()
    }

    pub fn try_next_session_lifecycle(
        &mut self,
        session: &LocalSessionClient,
    ) -> Option<SurfaceLifecycleEffect> {
        self.try_next_lifecycle_for_surface(session.surface_id())
    }

    pub fn try_next_passive_lifecycle(
        &mut self,
        session: &LocalPassiveSessionClient,
    ) -> Option<SurfaceLifecycleEffect> {
        self.try_next_lifecycle_for_surface(session.surface_id())
    }

    pub fn detach_passive(
        &mut self,
        passive: LocalPassiveSessionClient,
    ) -> Result<DetachSurfaceOutcome, (LocalPassiveSessionClient, ClientError)> {
        let outcome = self.connection.detach_surface(&passive.surface_id);
        if outcome.detached_surface.is_some() {
            self.purge_surface_buffers(&passive.surface_id);
            Ok(outcome)
        } else {
            Err((
                passive,
                ClientError::InvalidArgument("passive surface is not attached".to_string()),
            ))
        }
    }

    pub fn list_live_sessions(&self) -> Vec<LocalLiveSessionSummary> {
        self.connection
            .list_live_sessions()
            .into_iter()
            .map(|summary| LocalLiveSessionSummary {
                session_id: summary.session_id,
                surface_counts: summary.surface_counts,
            })
            .collect()
    }

    fn try_next_event_for_surface(&mut self, surface_id: &SurfaceId) -> Option<SessionEnvelope> {
        if let Some(envelope) = self.pop_buffered_event(surface_id) {
            return Some(envelope);
        }

        loop {
            let delivery = self.connection.events_mut().try_recv().ok()?;
            if &delivery.surface_id == surface_id {
                return Some(delivery.envelope);
            }
            self.event_buffers
                .entry(delivery.surface_id)
                .or_default()
                .push_back(delivery.envelope);
        }
    }

    async fn next_event_for_surface(&mut self, surface_id: &SurfaceId) -> Option<SessionEnvelope> {
        if let Some(envelope) = self.pop_buffered_event(surface_id) {
            return Some(envelope);
        }

        loop {
            let delivery = self.connection.events_mut().recv().await?;
            if &delivery.surface_id == surface_id {
                return Some(delivery.envelope);
            }
            self.event_buffers
                .entry(delivery.surface_id)
                .or_default()
                .push_back(delivery.envelope);
        }
    }

    fn pop_buffered_event(&mut self, surface_id: &SurfaceId) -> Option<SessionEnvelope> {
        let queue = self.event_buffers.get_mut(surface_id)?;
        let envelope = queue.pop_front();
        if queue.is_empty() {
            self.event_buffers.remove(surface_id);
        }
        envelope
    }

    fn try_next_request_for_surface(
        &mut self,
        surface_id: &SurfaceId,
    ) -> Option<ServerRequestDelivery> {
        if let Some(delivery) = Self::pop_buffered_delivery(&mut self.request_buffers, surface_id) {
            return Some(delivery);
        }

        loop {
            let delivery = self.connection.server_requests_mut().try_recv().ok()?;
            if &delivery.surface_id == surface_id {
                return Some(delivery);
            }
            self.request_buffers
                .entry(delivery.surface_id.clone())
                .or_default()
                .push_back(delivery);
        }
    }

    fn try_next_lifecycle_for_surface(
        &mut self,
        surface_id: &SurfaceId,
    ) -> Option<SurfaceLifecycleEffect> {
        if let Some(delivery) = Self::pop_buffered_delivery(&mut self.lifecycle_buffers, surface_id)
        {
            return Some(delivery);
        }

        loop {
            let delivery = self.connection.lifecycle_mut().try_recv().ok()?;
            if &delivery.surface_id == surface_id {
                return Some(delivery);
            }
            self.lifecycle_buffers
                .entry(delivery.surface_id.clone())
                .or_default()
                .push_back(delivery);
        }
    }

    fn pop_buffered_delivery<T>(
        buffers: &mut HashMap<SurfaceId, VecDeque<T>>,
        surface_id: &SurfaceId,
    ) -> Option<T> {
        let queue = buffers.get_mut(surface_id)?;
        let delivery = queue.pop_front();
        if queue.is_empty() {
            buffers.remove(surface_id);
        }
        delivery
    }
}
