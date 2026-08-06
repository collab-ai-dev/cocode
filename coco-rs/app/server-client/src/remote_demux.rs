pub struct RemoteEventDemux {
    events: mpsc::Receiver<RemoteJsonRpcEvent>,
    invalidator: crate::RemoteConnectionInvalidator,
    event_buffers: HashMap<SessionId, VecDeque<SessionEnvelope>>,
    lifecycle_buffers: HashMap<SessionId, VecDeque<SessionLifecycleEffect>>,
    server_requests: VecDeque<JsonRpcRequest>,
    notifications: VecDeque<JsonRpcNotification>,
    session_lags: HashMap<SessionId, RemoteSessionLag>,
    disconnected: bool,
    disconnect_reason: Option<RemoteDemuxDisconnectReason>,
}

pub struct RemoteSessionStream<'a> {
    demux: &'a mut RemoteEventDemux,
    session_id: SessionId,
}

pub struct RemoteOwnedSessionStream {
    demux: RemoteEventDemux,
    session_id: SessionId,
}

#[derive(Debug, Clone)]
pub enum RemoteJsonRpcEvent {
    SessionDelivery(Box<SessionDelivery>),
    SessionLifecycle(SessionLifecycleEffect),
    Notification(JsonRpcNotification),
    ServerRequest(JsonRpcRequest),
    Disconnected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteDemuxDisconnectReason {
    TransportClosed,
    EventChannelClosed,
    SlowConsumer {
        queue: &'static str,
        capacity: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "remote session {session_id} lagged on {queue} (capacity {capacity}); purge and resubscribe it"
)]
pub struct RemoteSessionLag {
    pub session_id: SessionId,
    pub queue: &'static str,
    pub capacity: usize,
}

type SessionPoll<T> = Result<Option<T>, RemoteSessionLag>;

impl RemoteEventDemux {
    pub(crate) fn new(
        events: mpsc::Receiver<RemoteJsonRpcEvent>,
        invalidator: crate::RemoteConnectionInvalidator,
    ) -> Self {
        Self {
            events,
            invalidator,
            event_buffers: HashMap::new(),
            lifecycle_buffers: HashMap::new(),
            server_requests: VecDeque::new(),
            notifications: VecDeque::new(),
            session_lags: HashMap::new(),
            disconnected: false,
            disconnect_reason: None,
        }
    }

    pub fn try_next_session_event(
        &mut self,
        session_id: &SessionId,
    ) -> SessionPoll<SessionEnvelope> {
        self.ensure_session_current(session_id)?;
        if let Some(envelope) = self.pop_buffered_event(session_id) {
            return Ok(Some(envelope));
        }

        loop {
            let Some(event) = self.next_remote_event() else {
                return Ok(None);
            };
            match event {
                RemoteJsonRpcEvent::SessionDelivery(delivery) => {
                    if &delivery.envelope.session_id == session_id {
                        return Ok(Some(delivery.envelope));
                    }
                    self.push_session_event(
                        delivery.envelope.session_id.clone(),
                        delivery.envelope,
                    );
                }
                event => self.buffer_non_session_event(event),
            }
            self.ensure_session_current(session_id)?;
        }
    }

    pub async fn next_session_event(
        &mut self,
        session_id: &SessionId,
    ) -> SessionPoll<SessionEnvelope> {
        self.ensure_session_current(session_id)?;
        if let Some(envelope) = self.pop_buffered_event(session_id) {
            return Ok(Some(envelope));
        }

        loop {
            let Some(event) = self.recv_remote_event().await else {
                return Ok(None);
            };
            match event {
                RemoteJsonRpcEvent::SessionDelivery(delivery) => {
                    if &delivery.envelope.session_id == session_id {
                        return Ok(Some(delivery.envelope));
                    }
                    self.push_session_event(
                        delivery.envelope.session_id.clone(),
                        delivery.envelope,
                    );
                }
                event => self.buffer_non_session_event(event),
            }
            self.ensure_session_current(session_id)?;
        }
    }

    pub fn try_next_lifecycle(
        &mut self,
        session_id: &SessionId,
    ) -> SessionPoll<SessionLifecycleEffect> {
        self.ensure_session_current(session_id)?;
        if let Some(delivery) = self.pop_buffered_lifecycle(session_id) {
            self.purge_on_session_ended(&delivery);
            return Ok(Some(delivery));
        }

        loop {
            let Some(event) = self.next_remote_event() else {
                return Ok(None);
            };
            match event {
                RemoteJsonRpcEvent::SessionLifecycle(delivery) => {
                    if lifecycle_session_id(&delivery) == session_id {
                        self.purge_on_session_ended(&delivery);
                        return Ok(Some(delivery));
                    }
                    self.push_session_lifecycle(lifecycle_session_id(&delivery).clone(), delivery);
                }
                event => self.buffer_non_lifecycle_event(event),
            }
            self.ensure_session_current(session_id)?;
        }
    }

    pub async fn next_lifecycle(
        &mut self,
        session_id: &SessionId,
    ) -> SessionPoll<SessionLifecycleEffect> {
        self.ensure_session_current(session_id)?;
        if let Some(delivery) = self.pop_buffered_lifecycle(session_id) {
            self.purge_on_session_ended(&delivery);
            return Ok(Some(delivery));
        }

        loop {
            let Some(event) = self.recv_remote_event().await else {
                return Ok(None);
            };
            match event {
                RemoteJsonRpcEvent::SessionLifecycle(delivery) => {
                    if lifecycle_session_id(&delivery) == session_id {
                        self.purge_on_session_ended(&delivery);
                        return Ok(Some(delivery));
                    }
                    self.push_session_lifecycle(lifecycle_session_id(&delivery).clone(), delivery);
                }
                event => self.buffer_non_lifecycle_event(event),
            }
            self.ensure_session_current(session_id)?;
        }
    }

    /// When a caller consumes a `SessionEnded`, drop that
    /// session's now-moot buffered deliveries.
    fn purge_on_session_ended(&mut self, delivery: &SessionLifecycleEffect) {
        if let SessionLifecycleEffectKind::SessionEnded { session_id } = &delivery.kind {
            self.purge_session(session_id);
        }
    }

    pub fn try_next_session_activation(
        &mut self,
        session_id: &SessionId,
    ) -> SessionPoll<SessionLifecycleEffect> {
        self.ensure_session_current(session_id)?;
        if let Some(delivery) = self.take_buffered_activation(session_id) {
            return Ok(Some(delivery));
        }
        loop {
            let Some(event) = self.next_remote_event() else {
                return Ok(None);
            };
            match event {
                RemoteJsonRpcEvent::SessionLifecycle(delivery) => {
                    if lifecycle_activates_session(&delivery, session_id) {
                        return Ok(Some(delivery));
                    }
                    self.push_session_lifecycle(lifecycle_session_id(&delivery).clone(), delivery);
                }
                event => self.buffer_non_lifecycle_event(event),
            }
            self.ensure_session_current(session_id)?;
        }
    }

    pub async fn next_session_activation(
        &mut self,
        session_id: &SessionId,
    ) -> SessionPoll<SessionLifecycleEffect> {
        self.ensure_session_current(session_id)?;
        if let Some(delivery) = self.take_buffered_activation(session_id) {
            return Ok(Some(delivery));
        }
        loop {
            let Some(event) = self.recv_remote_event().await else {
                return Ok(None);
            };
            match event {
                RemoteJsonRpcEvent::SessionLifecycle(delivery) => {
                    if lifecycle_activates_session(&delivery, session_id) {
                        return Ok(Some(delivery));
                    }
                    self.push_session_lifecycle(lifecycle_session_id(&delivery).clone(), delivery);
                }
                event => self.buffer_non_lifecycle_event(event),
            }
            self.ensure_session_current(session_id)?;
        }
    }

    /// Scan every buffered lifecycle queue for a delivery that activates
    /// `session_id`, removing it in place. Other demux accessors buffer
    /// activations they don't match; without this scan the waiter would block on
    /// `recv` forever while its activation sits in a sibling session's queue.
    fn take_buffered_activation(
        &mut self,
        session_id: &SessionId,
    ) -> Option<SessionLifecycleEffect> {
        let (buffer_session_id, pos) =
            self.lifecycle_buffers
                .iter()
                .find_map(|(buffer_session_id, queue)| {
                    let pos = queue
                        .iter()
                        .position(|delivery| lifecycle_activates_session(delivery, session_id))?;
                    Some((buffer_session_id.clone(), pos))
                })?;
        let queue = self.lifecycle_buffers.get_mut(&buffer_session_id)?;
        let delivery = queue.remove(pos);
        if queue.is_empty() {
            self.lifecycle_buffers.remove(&buffer_session_id);
        }
        delivery
    }

    pub fn try_next_server_request(&mut self) -> Option<JsonRpcRequest> {
        if let Some(request) = self.server_requests.pop_front() {
            return Some(request);
        }

        loop {
            match self.next_remote_event()? {
                RemoteJsonRpcEvent::ServerRequest(request) => return Some(request),
                event => self.buffer_non_server_request_event(event),
            }
        }
    }

    pub async fn next_server_request(&mut self) -> Option<JsonRpcRequest> {
        if let Some(request) = self.server_requests.pop_front() {
            return Some(request);
        }

        loop {
            match self.recv_remote_event().await? {
                RemoteJsonRpcEvent::ServerRequest(request) => return Some(request),
                event => self.buffer_non_server_request_event(event),
            }
        }
    }

    pub fn try_next_notification(&mut self) -> Option<JsonRpcNotification> {
        if let Some(notification) = self.notifications.pop_front() {
            return Some(notification);
        }

        loop {
            match self.next_remote_event()? {
                RemoteJsonRpcEvent::Notification(notification) => {
                    self.purge_cancelled_server_request(&notification);
                    return Some(notification);
                }
                event => self.buffer_non_notification_event(event),
            }
        }
    }

    pub async fn next_notification(&mut self) -> Option<JsonRpcNotification> {
        if let Some(notification) = self.notifications.pop_front() {
            return Some(notification);
        }

        loop {
            match self.recv_remote_event().await? {
                RemoteJsonRpcEvent::Notification(notification) => {
                    self.purge_cancelled_server_request(&notification);
                    return Some(notification);
                }
                event => self.buffer_non_notification_event(event),
            }
        }
    }

    pub fn is_disconnected(&self) -> bool {
        self.disconnected
    }

    pub fn disconnect_reason(&self) -> Option<&RemoteDemuxDisconnectReason> {
        self.disconnect_reason.as_ref()
    }

    /// Drop every buffered per-session queue for `session_id` (events +
    /// lifecycle) plus its lag marker. Call before resubscribing a lagged
    /// session, or after it is closed/detached/replaced, so stale deliveries do
    /// not linger. The connection-scoped `server_requests` / `notifications`
    /// queues are not session-keyed and are bounded separately.
    pub fn purge_session(&mut self, session_id: &SessionId) {
        self.event_buffers.remove(session_id);
        self.lifecycle_buffers.remove(session_id);
        self.session_lags.remove(session_id);
    }

    /// Server requests require a response, so overflow is fatal rather than a
    /// drop-oldest queue. Silently dropping one would leave the peer waiting
    /// forever and desynchronize request cancellation.
    fn push_server_request(&mut self, request: JsonRpcRequest) {
        if self.server_requests.len() >= MAX_BUFFERED_CONNECTION_QUEUE {
            tracing::warn!(
                cap = MAX_BUFFERED_CONNECTION_QUEUE,
                "remote demux server-request buffer full; disconnecting slow consumer"
            );
            self.disconnect_slow_consumer("server_requests", MAX_BUFFERED_CONNECTION_QUEUE);
            return;
        }
        self.server_requests.push_back(request);
    }

    /// Buffer an event for a sibling session. Overflow invalidates only that
    /// session stream; the caller can replay it without disrupting unrelated
    /// sessions or pending connection-level RPCs.
    fn push_session_event(&mut self, session_id: SessionId, envelope: SessionEnvelope) {
        if self.session_lags.contains_key(&session_id) {
            return;
        }
        if self
            .event_buffers
            .get(&session_id)
            .is_some_and(|queue| queue.len() >= MAX_BUFFERED_SESSION_QUEUE)
        {
            tracing::warn!(
                %session_id,
                cap = MAX_BUFFERED_SESSION_QUEUE,
                "remote demux per-session event buffer full; session must resubscribe"
            );
            self.mark_session_lag(session_id, "session_events", MAX_BUFFERED_SESSION_QUEUE);
            return;
        }
        self.event_buffers
            .entry(session_id)
            .or_default()
            .push_back(envelope);
    }

    /// Buffer a lifecycle effect for a sibling session. As with events, an
    /// overflow requires that session to replay without invalidating the
    /// connection shared by healthy sessions.
    fn push_session_lifecycle(&mut self, session_id: SessionId, effect: SessionLifecycleEffect) {
        let activated_session_id = lifecycle_activated_session_id(&effect)
            .filter(|activated| *activated != &session_id)
            .cloned();
        if self.session_lags.contains_key(&session_id) {
            if let Some(activated_session_id) = activated_session_id {
                self.mark_session_lag(
                    activated_session_id,
                    "session_lifecycle",
                    MAX_BUFFERED_SESSION_QUEUE,
                );
            }
            return;
        }
        if self
            .lifecycle_buffers
            .get(&session_id)
            .is_some_and(|queue| queue.len() >= MAX_BUFFERED_SESSION_QUEUE)
        {
            tracing::warn!(
                %session_id,
                cap = MAX_BUFFERED_SESSION_QUEUE,
                "remote demux per-session lifecycle buffer full; session must resubscribe"
            );
            self.mark_session_lag(session_id, "session_lifecycle", MAX_BUFFERED_SESSION_QUEUE);
            if let Some(activated_session_id) = activated_session_id {
                self.mark_session_lag(
                    activated_session_id,
                    "session_lifecycle",
                    MAX_BUFFERED_SESSION_QUEUE,
                );
            }
            return;
        }
        self.lifecycle_buffers
            .entry(session_id)
            .or_default()
            .push_back(effect);
    }

    /// Buffer a raw notification, dropping the oldest with a warning if the
    /// connection-scoped queue is at its cap.
    fn push_notification(&mut self, notification: JsonRpcNotification) {
        self.purge_cancelled_server_request(&notification);
        if self.notifications.len() >= MAX_BUFFERED_CONNECTION_QUEUE {
            self.notifications.pop_front();
            tracing::warn!(
                cap = MAX_BUFFERED_CONNECTION_QUEUE,
                "remote demux notification buffer full; dropping oldest"
            );
        }
        self.notifications.push_back(notification);
    }

    fn purge_cancelled_server_request(&mut self, notification: &JsonRpcNotification) {
        if notification.method != "control/cancelRequest" {
            return;
        }
        let Some(request_id) = notification
            .params
            .as_ref()
            .and_then(|params| params.get("request_id"))
            .and_then(serde_json::Value::as_str)
        else {
            return;
        };
        self.server_requests.retain(|request| match &request.id {
            coco_app_server_transport::JsonRpcId::String(id) => id != request_id,
            coco_app_server_transport::JsonRpcId::Number(id) => id.to_string() != request_id,
            coco_app_server_transport::JsonRpcId::Null => true,
        });
    }

    pub fn session_stream(&mut self, session_id: SessionId) -> RemoteSessionStream<'_> {
        RemoteSessionStream {
            demux: self,
            session_id,
        }
    }

    pub fn into_session_stream(self, session_id: SessionId) -> RemoteOwnedSessionStream {
        RemoteOwnedSessionStream {
            demux: self,
            session_id,
        }
    }

    fn next_remote_event(&mut self) -> Option<RemoteJsonRpcEvent> {
        if self.disconnected {
            return None;
        }
        match self.events.try_recv() {
            Ok(RemoteJsonRpcEvent::Disconnected) => {
                self.mark_disconnected(RemoteDemuxDisconnectReason::TransportClosed);
                None
            }
            Ok(event) => Some(event),
            Err(mpsc::error::TryRecvError::Empty) => None,
            Err(mpsc::error::TryRecvError::Disconnected) => {
                self.mark_disconnected(RemoteDemuxDisconnectReason::EventChannelClosed);
                None
            }
        }
    }

    async fn recv_remote_event(&mut self) -> Option<RemoteJsonRpcEvent> {
        if self.disconnected {
            return None;
        }
        match self.events.recv().await {
            Some(RemoteJsonRpcEvent::Disconnected) => {
                self.mark_disconnected(RemoteDemuxDisconnectReason::TransportClosed);
                None
            }
            Some(event) => Some(event),
            None => {
                self.mark_disconnected(RemoteDemuxDisconnectReason::EventChannelClosed);
                None
            }
        }
    }

    fn pop_buffered_event(&mut self, session_id: &SessionId) -> Option<SessionEnvelope> {
        let queue = self.event_buffers.get_mut(session_id)?;
        let envelope = queue.pop_front();
        if queue.is_empty() {
            self.event_buffers.remove(session_id);
        }
        envelope
    }

    fn pop_buffered_lifecycle(&mut self, session_id: &SessionId) -> Option<SessionLifecycleEffect> {
        let queue = self.lifecycle_buffers.get_mut(session_id)?;
        let delivery = queue.pop_front();
        if queue.is_empty() {
            self.lifecycle_buffers.remove(session_id);
        }
        delivery
    }

    fn buffer_non_session_event(&mut self, event: RemoteJsonRpcEvent) {
        match event {
            RemoteJsonRpcEvent::SessionLifecycle(delivery) => {
                self.push_session_lifecycle(lifecycle_session_id(&delivery).clone(), delivery);
            }
            other => self.buffer_common_event(other),
        }
    }

    fn buffer_non_lifecycle_event(&mut self, event: RemoteJsonRpcEvent) {
        match event {
            RemoteJsonRpcEvent::SessionDelivery(delivery) => {
                self.push_session_event(delivery.envelope.session_id.clone(), delivery.envelope);
            }
            other => self.buffer_common_event(other),
        }
    }

    fn buffer_non_server_request_event(&mut self, event: RemoteJsonRpcEvent) {
        match event {
            RemoteJsonRpcEvent::SessionDelivery(delivery) => {
                self.push_session_event(delivery.envelope.session_id.clone(), delivery.envelope);
            }
            RemoteJsonRpcEvent::SessionLifecycle(delivery) => {
                self.push_session_lifecycle(lifecycle_session_id(&delivery).clone(), delivery);
            }
            other => self.buffer_common_event(other),
        }
    }

    fn buffer_non_notification_event(&mut self, event: RemoteJsonRpcEvent) {
        match event {
            RemoteJsonRpcEvent::SessionDelivery(delivery) => {
                self.push_session_event(delivery.envelope.session_id.clone(), delivery.envelope);
            }
            RemoteJsonRpcEvent::SessionLifecycle(delivery) => {
                self.push_session_lifecycle(lifecycle_session_id(&delivery).clone(), delivery);
            }
            RemoteJsonRpcEvent::ServerRequest(request) => {
                self.push_server_request(request);
            }
            RemoteJsonRpcEvent::Disconnected => {
                self.mark_disconnected(RemoteDemuxDisconnectReason::TransportClosed);
            }
            RemoteJsonRpcEvent::Notification(_) => {}
        }
    }

    fn buffer_common_event(&mut self, event: RemoteJsonRpcEvent) {
        match event {
            RemoteJsonRpcEvent::ServerRequest(request) => {
                self.push_server_request(request);
            }
            RemoteJsonRpcEvent::Notification(notification) => {
                self.push_notification(notification);
            }
            RemoteJsonRpcEvent::Disconnected => {
                self.mark_disconnected(RemoteDemuxDisconnectReason::TransportClosed);
            }
            RemoteJsonRpcEvent::SessionDelivery(_) | RemoteJsonRpcEvent::SessionLifecycle(_) => {}
        }
    }

    fn disconnect_slow_consumer(&mut self, queue: &'static str, capacity: usize) {
        self.events.close();
        self.invalidator.invalidate();
        self.mark_disconnected(RemoteDemuxDisconnectReason::SlowConsumer { queue, capacity });
        self.event_buffers.clear();
        self.lifecycle_buffers.clear();
        self.session_lags.clear();
    }

    fn mark_disconnected(&mut self, reason: RemoteDemuxDisconnectReason) {
        self.disconnected = true;
        self.disconnect_reason.get_or_insert(reason);
        // Connection-scoped work cannot be completed after EOF. Per-session
        // deliveries remain drainable because they are already self-contained.
        self.server_requests.clear();
        self.notifications.clear();
    }

    fn ensure_session_current(&self, session_id: &SessionId) -> Result<(), RemoteSessionLag> {
        self.session_lags
            .get(session_id)
            .cloned()
            .map_or(Ok(()), Err)
    }

    fn mark_session_lag(&mut self, session_id: SessionId, queue: &'static str, capacity: usize) {
        self.event_buffers.remove(&session_id);
        self.lifecycle_buffers.remove(&session_id);
        self.session_lags.insert(
            session_id.clone(),
            RemoteSessionLag {
                session_id,
                queue,
                capacity,
            },
        );
    }
}

impl RemoteSessionStream<'_> {
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn try_next_event(&mut self) -> SessionPoll<SessionEnvelope> {
        self.demux.try_next_session_event(&self.session_id)
    }

    pub async fn next_event(&mut self) -> SessionPoll<SessionEnvelope> {
        self.demux.next_session_event(&self.session_id).await
    }

    pub fn try_next_lifecycle(&mut self) -> SessionPoll<SessionLifecycleEffect> {
        self.demux.try_next_lifecycle(&self.session_id)
    }

    pub async fn next_lifecycle(&mut self) -> SessionPoll<SessionLifecycleEffect> {
        self.demux.next_lifecycle(&self.session_id).await
    }
}

impl RemoteOwnedSessionStream {
    pub fn new(demux: RemoteEventDemux, session_id: SessionId) -> Self {
        Self { demux, session_id }
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn demux_mut(&mut self) -> &mut RemoteEventDemux {
        &mut self.demux
    }

    pub fn into_demux(self) -> RemoteEventDemux {
        self.demux
    }

    pub fn try_next_event(&mut self) -> SessionPoll<SessionEnvelope> {
        self.demux.try_next_session_event(&self.session_id)
    }

    pub async fn next_event(&mut self) -> SessionPoll<SessionEnvelope> {
        self.demux.next_session_event(&self.session_id).await
    }

    pub fn try_next_lifecycle(&mut self) -> SessionPoll<SessionLifecycleEffect> {
        self.demux.try_next_lifecycle(&self.session_id)
    }

    pub async fn next_lifecycle(&mut self) -> SessionPoll<SessionLifecycleEffect> {
        self.demux.next_lifecycle(&self.session_id).await
    }
}
pub(super) fn remote_event_from_notification(
    notification: JsonRpcNotification,
) -> Option<RemoteJsonRpcEvent> {
    match notification.method.as_str() {
        coco_event_types::SESSION_EVENT_METHOD => {
            match decode_session_delivery_notification(notification.params) {
                Ok(delivery) => Some(RemoteJsonRpcEvent::SessionDelivery(Box::new(delivery))),
                Err(error) => {
                    tracing::warn!(%error, "dropping undecodable session/event notification");
                    None
                }
            }
        }
        coco_event_types::SESSION_LIFECYCLE_METHOD => {
            match decode_lifecycle_notification(notification.params) {
                Ok(delivery) => Some(RemoteJsonRpcEvent::SessionLifecycle(delivery)),
                Err(error) => {
                    tracing::warn!(%error, "dropping undecodable session/lifecycle notification");
                    None
                }
            }
        }
        _ => Some(RemoteJsonRpcEvent::Notification(notification)),
    }
}

fn decode_session_delivery_notification(
    params: Option<serde_json::Value>,
) -> Result<SessionDelivery, ClientError> {
    let mut params = object_params(params, "session/event")?;
    let mut envelope = take_object_field(&mut params, "envelope", "session/event")?;
    let session_id = take_field(&mut envelope, "session_id", "session/event envelope")?;
    let agent_id = take_optional_field(&mut envelope, "agent_id", "session/event envelope")?;
    let turn_id = take_optional_field(&mut envelope, "turn_id", "session/event envelope")?;
    let session_seq = take_optional_field(&mut envelope, "session_seq", "session/event envelope")?;
    let event = decode_core_event(take_object_field(
        &mut envelope,
        "event",
        "session/event envelope",
    )?)?;
    Ok(SessionDelivery {
        envelope: SessionEnvelope {
            session_id,
            agent_id,
            turn_id,
            session_seq,
            event,
        },
    })
}

fn decode_lifecycle_notification(
    params: Option<serde_json::Value>,
) -> Result<SessionLifecycleEffect, ClientError> {
    let mut params = object_params(params, "session/lifecycle")?;
    let mut effect = take_object_field(&mut params, "effect", "session/lifecycle")?;
    let effect_type: String = take_field(&mut effect, "type", "session/lifecycle effect")?;
    let kind = match effect_type.as_str() {
        "session_started" => SessionLifecycleEffectKind::SessionStarted {
            session_id: take_field(&mut effect, "session_id", "session/lifecycle effect")?,
        },
        "session_replaced" => SessionLifecycleEffectKind::SessionReplaced {
            old_session_id: take_field(&mut effect, "old_session_id", "session/lifecycle effect")?,
            new_session_id: take_field(&mut effect, "new_session_id", "session/lifecycle effect")?,
        },
        "session_ended" => SessionLifecycleEffectKind::SessionEnded {
            session_id: take_field(&mut effect, "session_id", "session/lifecycle effect")?,
        },
        other => {
            return Err(ClientError::InvalidArgument(format!(
                "unknown session/lifecycle effect type: {other}"
            )));
        }
    };
    Ok(SessionLifecycleEffect { kind })
}

fn lifecycle_session_id(delivery: &SessionLifecycleEffect) -> &SessionId {
    match &delivery.kind {
        SessionLifecycleEffectKind::SessionStarted { session_id }
        | SessionLifecycleEffectKind::SessionEnded { session_id } => session_id,
        SessionLifecycleEffectKind::SessionReplaced { old_session_id, .. } => old_session_id,
    }
}

fn lifecycle_activates_session(delivery: &SessionLifecycleEffect, session_id: &SessionId) -> bool {
    match &delivery.kind {
        SessionLifecycleEffectKind::SessionStarted {
            session_id: started,
        } => started == session_id,
        SessionLifecycleEffectKind::SessionReplaced { new_session_id, .. } => {
            new_session_id == session_id
        }
        SessionLifecycleEffectKind::SessionEnded { .. } => false,
    }
}

fn lifecycle_activated_session_id(delivery: &SessionLifecycleEffect) -> Option<&SessionId> {
    match &delivery.kind {
        SessionLifecycleEffectKind::SessionStarted { session_id } => Some(session_id),
        SessionLifecycleEffectKind::SessionReplaced { new_session_id, .. } => Some(new_session_id),
        SessionLifecycleEffectKind::SessionEnded { .. } => None,
    }
}
pub(super) fn decode_session_subscribe_envelope(
    envelope: SessionSubscribeEnvelope,
) -> Result<SessionEnvelope, ClientError> {
    let event = match envelope.event {
        serde_json::Value::Object(event) => event,
        _ => {
            return Err(ClientError::InvalidArgument(
                "session/subscribe replay event must be an object".to_string(),
            ));
        }
    };
    Ok(SessionEnvelope {
        session_id: envelope.session_id,
        agent_id: envelope
            .agent_id
            .map(AgentId::try_new)
            .transpose()
            .map_err(|error| {
                ClientError::InvalidArgument(format!("invalid replay agent_id: {error}"))
            })?,
        turn_id: envelope.turn_id,
        session_seq: envelope.session_seq,
        event: decode_core_event(event)?,
    })
}

fn decode_core_event(
    mut event: serde_json::Map<String, serde_json::Value>,
) -> Result<CoreEvent, ClientError> {
    let layer: String = take_field(&mut event, "layer", "session/event core event")?;
    let payload = event
        .remove("payload")
        .ok_or_else(|| ClientError::InvalidArgument("missing session/event payload".to_string()))?;
    let layer = layer
        .parse::<coco_event_types::EventLayer>()
        .map_err(|()| {
            ClientError::InvalidArgument(format!("unknown session/event layer: {layer}"))
        })?;
    match layer {
        coco_event_types::EventLayer::Protocol => {
            serde_json::from_value::<ServerNotification>(payload)
                .map(CoreEvent::Protocol)
                .map_err(|error| {
                    ClientError::InvalidArgument(format!("invalid protocol event: {error}"))
                })
        }
        coco_event_types::EventLayer::Stream => serde_json::from_value::<AgentStreamEvent>(payload)
            .map(CoreEvent::Stream)
            .map_err(|error| {
                ClientError::InvalidArgument(format!("invalid stream event: {error}"))
            }),
        coco_event_types::EventLayer::Tui => serde_json::from_value::<TuiOnlyEvent>(payload)
            .map(CoreEvent::Tui)
            .map_err(|error| ClientError::InvalidArgument(format!("invalid tui event: {error}"))),
    }
}

fn object_params(
    params: Option<serde_json::Value>,
    context: &str,
) -> Result<serde_json::Map<String, serde_json::Value>, ClientError> {
    match params {
        Some(serde_json::Value::Object(object)) => Ok(object),
        _ => Err(ClientError::InvalidArgument(format!(
            "{context} params must be an object"
        ))),
    }
}

fn take_object_field(
    object: &mut serde_json::Map<String, serde_json::Value>,
    field: &str,
    context: &str,
) -> Result<serde_json::Map<String, serde_json::Value>, ClientError> {
    match object.remove(field) {
        Some(serde_json::Value::Object(object)) => Ok(object),
        _ => Err(ClientError::InvalidArgument(format!(
            "missing or invalid {context}.{field}"
        ))),
    }
}

fn take_field<T>(
    object: &mut serde_json::Map<String, serde_json::Value>,
    field: &str,
    context: &str,
) -> Result<T, ClientError>
where
    T: serde::de::DeserializeOwned,
{
    let value = object
        .remove(field)
        .ok_or_else(|| ClientError::InvalidArgument(format!("missing {context}.{field}")))?;
    serde_json::from_value(value).map_err(|error| {
        ClientError::InvalidArgument(format!("invalid {context}.{field}: {error}"))
    })
}

fn take_optional_field<T>(
    object: &mut serde_json::Map<String, serde_json::Value>,
    field: &str,
    context: &str,
) -> Result<Option<T>, ClientError>
where
    T: serde::de::DeserializeOwned,
{
    let Some(value) = object.remove(field) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    serde_json::from_value(value).map(Some).map_err(|error| {
        ClientError::InvalidArgument(format!("invalid {context}.{field}: {error}"))
    })
}
use std::collections::HashMap;
use std::collections::VecDeque;

use coco_app_server_transport::JsonRpcNotification;
use coco_app_server_transport::JsonRpcRequest;
use coco_event_types::AgentStreamEvent;
use coco_event_types::CoreEvent;
use coco_event_types::ServerNotification;
use coco_event_types::SessionDelivery;
use coco_event_types::SessionEnvelope;
use coco_event_types::SessionLifecycleEffect;
use coco_event_types::SessionLifecycleEffectKind;
use coco_event_types::TuiOnlyEvent;
use coco_types::AgentId;
use coco_types::SessionId;
use coco_types::SessionSubscribeEnvelope;
use tokio::sync::mpsc;

use super::ClientError;
use super::{MAX_BUFFERED_CONNECTION_QUEUE, MAX_BUFFERED_SESSION_QUEUE};
