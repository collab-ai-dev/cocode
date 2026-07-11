pub struct RemoteEventDemux {
    events: mpsc::Receiver<RemoteJsonRpcEvent>,
    event_buffers: HashMap<SurfaceId, VecDeque<SessionEnvelope>>,
    lifecycle_buffers: HashMap<SurfaceId, VecDeque<SurfaceLifecycleEffect>>,
    server_requests: VecDeque<JsonRpcRequest>,
    notifications: VecDeque<JsonRpcNotification>,
    disconnected: bool,
}

pub struct RemoteSurfaceStream<'a> {
    demux: &'a mut RemoteEventDemux,
    surface_id: SurfaceId,
}

pub struct RemoteOwnedSurfaceStream {
    demux: RemoteEventDemux,
    surface_id: SurfaceId,
}

#[derive(Debug, Clone)]
pub enum RemoteJsonRpcEvent {
    SurfaceDelivery(Box<SurfaceDelivery>),
    SurfaceLifecycle(SurfaceLifecycleEffect),
    Notification(JsonRpcNotification),
    ServerRequest(JsonRpcRequest),
    Disconnected,
}
impl RemoteEventDemux {
    pub fn new(events: mpsc::Receiver<RemoteJsonRpcEvent>) -> Self {
        Self {
            events,
            event_buffers: HashMap::new(),
            lifecycle_buffers: HashMap::new(),
            server_requests: VecDeque::new(),
            notifications: VecDeque::new(),
            disconnected: false,
        }
    }

    pub fn try_next_surface_event(&mut self, surface_id: &SurfaceId) -> Option<SessionEnvelope> {
        if let Some(envelope) = self.pop_buffered_event(surface_id) {
            return Some(envelope);
        }

        loop {
            match self.next_remote_event()? {
                RemoteJsonRpcEvent::SurfaceDelivery(delivery) => {
                    if &delivery.surface_id == surface_id {
                        return Some(delivery.envelope);
                    }
                    self.event_buffers
                        .entry(delivery.surface_id.clone())
                        .or_default()
                        .push_back(delivery.envelope);
                }
                event => self.buffer_non_surface_event(event),
            }
        }
    }

    pub async fn next_surface_event(&mut self, surface_id: &SurfaceId) -> Option<SessionEnvelope> {
        if let Some(envelope) = self.pop_buffered_event(surface_id) {
            return Some(envelope);
        }

        loop {
            match self.recv_remote_event().await? {
                RemoteJsonRpcEvent::SurfaceDelivery(delivery) => {
                    if &delivery.surface_id == surface_id {
                        return Some(delivery.envelope);
                    }
                    self.event_buffers
                        .entry(delivery.surface_id.clone())
                        .or_default()
                        .push_back(delivery.envelope);
                }
                event => self.buffer_non_surface_event(event),
            }
        }
    }

    pub fn try_next_lifecycle(&mut self, surface_id: &SurfaceId) -> Option<SurfaceLifecycleEffect> {
        if let Some(delivery) = self.pop_buffered_lifecycle(surface_id) {
            self.purge_on_session_ended(&delivery);
            return Some(delivery);
        }

        loop {
            match self.next_remote_event()? {
                RemoteJsonRpcEvent::SurfaceLifecycle(delivery) => {
                    if &delivery.surface_id == surface_id {
                        self.purge_on_session_ended(&delivery);
                        return Some(delivery);
                    }
                    self.lifecycle_buffers
                        .entry(delivery.surface_id.clone())
                        .or_default()
                        .push_back(delivery);
                }
                event => self.buffer_non_lifecycle_event(event),
            }
        }
    }

    pub async fn next_lifecycle(
        &mut self,
        surface_id: &SurfaceId,
    ) -> Option<SurfaceLifecycleEffect> {
        if let Some(delivery) = self.pop_buffered_lifecycle(surface_id) {
            self.purge_on_session_ended(&delivery);
            return Some(delivery);
        }

        loop {
            match self.recv_remote_event().await? {
                RemoteJsonRpcEvent::SurfaceLifecycle(delivery) => {
                    if &delivery.surface_id == surface_id {
                        self.purge_on_session_ended(&delivery);
                        return Some(delivery);
                    }
                    self.lifecycle_buffers
                        .entry(delivery.surface_id.clone())
                        .or_default()
                        .push_back(delivery);
                }
                event => self.buffer_non_lifecycle_event(event),
            }
        }
    }

    /// When a caller consumes a `SessionEnded` for its surface, drop that
    /// surface's now-moot buffered deliveries.
    fn purge_on_session_ended(&mut self, delivery: &SurfaceLifecycleEffect) {
        if matches!(
            delivery.kind,
            SurfaceLifecycleEffectKind::SessionEnded { .. }
        ) {
            self.purge_surface(&delivery.surface_id);
        }
    }

    pub fn try_next_session_activation(
        &mut self,
        session_id: &SessionId,
    ) -> Option<SurfaceLifecycleEffect> {
        if let Some(delivery) = self.take_buffered_activation(session_id) {
            return Some(delivery);
        }
        loop {
            match self.next_remote_event()? {
                RemoteJsonRpcEvent::SurfaceLifecycle(delivery) => {
                    if lifecycle_activates_session(&delivery, session_id) {
                        return Some(delivery);
                    }
                    self.lifecycle_buffers
                        .entry(delivery.surface_id.clone())
                        .or_default()
                        .push_back(delivery);
                }
                event => self.buffer_non_lifecycle_event(event),
            }
        }
    }

    pub async fn next_session_activation(
        &mut self,
        session_id: &SessionId,
    ) -> Option<SurfaceLifecycleEffect> {
        if let Some(delivery) = self.take_buffered_activation(session_id) {
            return Some(delivery);
        }
        loop {
            match self.recv_remote_event().await? {
                RemoteJsonRpcEvent::SurfaceLifecycle(delivery) => {
                    if lifecycle_activates_session(&delivery, session_id) {
                        return Some(delivery);
                    }
                    self.lifecycle_buffers
                        .entry(delivery.surface_id.clone())
                        .or_default()
                        .push_back(delivery);
                }
                event => self.buffer_non_lifecycle_event(event),
            }
        }
    }

    /// Scan every buffered lifecycle queue for a delivery that activates
    /// `session_id`, removing it in place. Other demux accessors buffer
    /// activations they don't match; without this scan the waiter would block on
    /// `recv` forever while its activation sits in a sibling surface's queue.
    fn take_buffered_activation(
        &mut self,
        session_id: &SessionId,
    ) -> Option<SurfaceLifecycleEffect> {
        let (surface_id, pos) = self
            .lifecycle_buffers
            .iter()
            .find_map(|(surface_id, queue)| {
                let pos = queue
                    .iter()
                    .position(|delivery| lifecycle_activates_session(delivery, session_id))?;
                Some((surface_id.clone(), pos))
            })?;
        let queue = self.lifecycle_buffers.get_mut(&surface_id)?;
        let delivery = queue.remove(pos);
        if queue.is_empty() {
            self.lifecycle_buffers.remove(&surface_id);
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
                RemoteJsonRpcEvent::Notification(notification) => return Some(notification),
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
                RemoteJsonRpcEvent::Notification(notification) => return Some(notification),
                event => self.buffer_non_notification_event(event),
            }
        }
    }

    pub fn is_disconnected(&self) -> bool {
        self.disconnected
    }

    /// Drop every buffered per-surface queue for `surface_id` (events +
    /// lifecycle). Call after the surface is closed/detached/replaced so stale
    /// deliveries do not linger. The connection-scoped `server_requests` /
    /// `notifications` queues are not surface-keyed and are bounded separately.
    pub fn purge_surface(&mut self, surface_id: &SurfaceId) {
        self.event_buffers.remove(surface_id);
        self.lifecycle_buffers.remove(surface_id);
    }

    /// Buffer a server request, dropping the oldest with a warning if the
    /// connection-scoped queue is at its cap.
    fn push_server_request(&mut self, request: JsonRpcRequest) {
        if self.server_requests.len() >= MAX_BUFFERED_CONNECTION_QUEUE {
            self.server_requests.pop_front();
            tracing::warn!(
                cap = MAX_BUFFERED_CONNECTION_QUEUE,
                "remote demux server-request buffer full; dropping oldest"
            );
        }
        self.server_requests.push_back(request);
    }

    /// Buffer a raw notification, dropping the oldest with a warning if the
    /// connection-scoped queue is at its cap.
    fn push_notification(&mut self, notification: JsonRpcNotification) {
        if self.notifications.len() >= MAX_BUFFERED_CONNECTION_QUEUE {
            self.notifications.pop_front();
            tracing::warn!(
                cap = MAX_BUFFERED_CONNECTION_QUEUE,
                "remote demux notification buffer full; dropping oldest"
            );
        }
        self.notifications.push_back(notification);
    }

    pub fn surface_stream(&mut self, surface_id: SurfaceId) -> RemoteSurfaceStream<'_> {
        RemoteSurfaceStream {
            demux: self,
            surface_id,
        }
    }

    pub fn into_surface_stream(self, surface_id: SurfaceId) -> RemoteOwnedSurfaceStream {
        RemoteOwnedSurfaceStream {
            demux: self,
            surface_id,
        }
    }

    fn next_remote_event(&mut self) -> Option<RemoteJsonRpcEvent> {
        match self.events.try_recv() {
            Ok(RemoteJsonRpcEvent::Disconnected) => {
                self.disconnected = true;
                None
            }
            Ok(event) => Some(event),
            Err(mpsc::error::TryRecvError::Empty) => None,
            Err(mpsc::error::TryRecvError::Disconnected) => {
                self.disconnected = true;
                None
            }
        }
    }

    async fn recv_remote_event(&mut self) -> Option<RemoteJsonRpcEvent> {
        match self.events.recv().await {
            Some(RemoteJsonRpcEvent::Disconnected) => {
                self.disconnected = true;
                None
            }
            Some(event) => Some(event),
            None => {
                self.disconnected = true;
                None
            }
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

    fn pop_buffered_lifecycle(&mut self, surface_id: &SurfaceId) -> Option<SurfaceLifecycleEffect> {
        let queue = self.lifecycle_buffers.get_mut(surface_id)?;
        let delivery = queue.pop_front();
        if queue.is_empty() {
            self.lifecycle_buffers.remove(surface_id);
        }
        delivery
    }

    fn buffer_non_surface_event(&mut self, event: RemoteJsonRpcEvent) {
        match event {
            RemoteJsonRpcEvent::SurfaceLifecycle(delivery) => {
                self.lifecycle_buffers
                    .entry(delivery.surface_id.clone())
                    .or_default()
                    .push_back(delivery);
            }
            other => self.buffer_common_event(other),
        }
    }

    fn buffer_non_lifecycle_event(&mut self, event: RemoteJsonRpcEvent) {
        match event {
            RemoteJsonRpcEvent::SurfaceDelivery(delivery) => {
                self.event_buffers
                    .entry(delivery.surface_id.clone())
                    .or_default()
                    .push_back(delivery.envelope);
            }
            other => self.buffer_common_event(other),
        }
    }

    fn buffer_non_server_request_event(&mut self, event: RemoteJsonRpcEvent) {
        match event {
            RemoteJsonRpcEvent::SurfaceDelivery(delivery) => {
                self.event_buffers
                    .entry(delivery.surface_id.clone())
                    .or_default()
                    .push_back(delivery.envelope);
            }
            RemoteJsonRpcEvent::SurfaceLifecycle(delivery) => {
                self.lifecycle_buffers
                    .entry(delivery.surface_id.clone())
                    .or_default()
                    .push_back(delivery);
            }
            other => self.buffer_common_event(other),
        }
    }

    fn buffer_non_notification_event(&mut self, event: RemoteJsonRpcEvent) {
        match event {
            RemoteJsonRpcEvent::SurfaceDelivery(delivery) => {
                self.event_buffers
                    .entry(delivery.surface_id.clone())
                    .or_default()
                    .push_back(delivery.envelope);
            }
            RemoteJsonRpcEvent::SurfaceLifecycle(delivery) => {
                self.lifecycle_buffers
                    .entry(delivery.surface_id.clone())
                    .or_default()
                    .push_back(delivery);
            }
            RemoteJsonRpcEvent::ServerRequest(request) => {
                self.push_server_request(request);
            }
            RemoteJsonRpcEvent::Disconnected => {
                self.disconnected = true;
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
                self.disconnected = true;
            }
            RemoteJsonRpcEvent::SurfaceDelivery(_) | RemoteJsonRpcEvent::SurfaceLifecycle(_) => {}
        }
    }
}

impl RemoteSurfaceStream<'_> {
    pub fn surface_id(&self) -> &SurfaceId {
        &self.surface_id
    }

    pub fn try_next_event(&mut self) -> Option<SessionEnvelope> {
        self.demux.try_next_surface_event(&self.surface_id)
    }

    pub async fn next_event(&mut self) -> Option<SessionEnvelope> {
        self.demux.next_surface_event(&self.surface_id).await
    }

    pub fn try_next_lifecycle(&mut self) -> Option<SurfaceLifecycleEffect> {
        self.demux.try_next_lifecycle(&self.surface_id)
    }

    pub async fn next_lifecycle(&mut self) -> Option<SurfaceLifecycleEffect> {
        self.demux.next_lifecycle(&self.surface_id).await
    }
}

impl RemoteOwnedSurfaceStream {
    pub fn new(demux: RemoteEventDemux, surface_id: SurfaceId) -> Self {
        Self { demux, surface_id }
    }

    pub fn surface_id(&self) -> &SurfaceId {
        &self.surface_id
    }

    pub fn demux_mut(&mut self) -> &mut RemoteEventDemux {
        &mut self.demux
    }

    pub fn into_demux(self) -> RemoteEventDemux {
        self.demux
    }

    pub fn try_next_event(&mut self) -> Option<SessionEnvelope> {
        self.demux.try_next_surface_event(&self.surface_id)
    }

    pub async fn next_event(&mut self) -> Option<SessionEnvelope> {
        self.demux.next_surface_event(&self.surface_id).await
    }

    pub fn try_next_lifecycle(&mut self) -> Option<SurfaceLifecycleEffect> {
        self.demux.try_next_lifecycle(&self.surface_id)
    }

    pub async fn next_lifecycle(&mut self) -> Option<SurfaceLifecycleEffect> {
        self.demux.next_lifecycle(&self.surface_id).await
    }
}
pub(super) fn remote_event_from_notification(
    notification: JsonRpcNotification,
) -> Option<RemoteJsonRpcEvent> {
    match notification.method.as_str() {
        "session/event" => match decode_surface_delivery_notification(notification.params) {
            Ok(delivery) => Some(RemoteJsonRpcEvent::SurfaceDelivery(Box::new(delivery))),
            Err(error) => {
                tracing::warn!(%error, "dropping undecodable session/event notification");
                None
            }
        },
        "session/lifecycle" => match decode_lifecycle_notification(notification.params) {
            Ok(delivery) => Some(RemoteJsonRpcEvent::SurfaceLifecycle(delivery)),
            Err(error) => {
                tracing::warn!(%error, "dropping undecodable session/lifecycle notification");
                None
            }
        },
        _ => Some(RemoteJsonRpcEvent::Notification(notification)),
    }
}

fn decode_surface_delivery_notification(
    params: Option<serde_json::Value>,
) -> Result<SurfaceDelivery, ClientError> {
    let mut params = object_params(params, "session/event")?;
    let surface_id = take_field(&mut params, "surface_id", "session/event")?;
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
    Ok(SurfaceDelivery {
        surface_id,
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
) -> Result<SurfaceLifecycleEffect, ClientError> {
    let mut params = object_params(params, "session/lifecycle")?;
    let surface_id: SurfaceId = take_field(&mut params, "surface_id", "session/lifecycle")?;
    let mut effect = take_object_field(&mut params, "effect", "session/lifecycle")?;
    let effect_type: String = take_field(&mut effect, "type", "session/lifecycle effect")?;
    let kind = match effect_type.as_str() {
        "session_started" => SurfaceLifecycleEffectKind::SessionStarted {
            session_id: take_field(&mut effect, "session_id", "session/lifecycle effect")?,
        },
        "session_replaced" => SurfaceLifecycleEffectKind::SessionReplaced {
            old_session_id: take_field(&mut effect, "old_session_id", "session/lifecycle effect")?,
            new_session_id: take_field(&mut effect, "new_session_id", "session/lifecycle effect")?,
        },
        "session_ended" => SurfaceLifecycleEffectKind::SessionEnded {
            session_id: take_field(&mut effect, "session_id", "session/lifecycle effect")?,
        },
        other => {
            return Err(ClientError::InvalidArgument(format!(
                "unknown session/lifecycle effect type: {other}"
            )));
        }
    };
    Ok(SurfaceLifecycleEffect { surface_id, kind })
}

fn lifecycle_activates_session(delivery: &SurfaceLifecycleEffect, session_id: &SessionId) -> bool {
    match &delivery.kind {
        SurfaceLifecycleEffectKind::SessionStarted {
            session_id: started,
        } => started == session_id,
        SurfaceLifecycleEffectKind::SessionReplaced { new_session_id, .. } => {
            new_session_id == session_id
        }
        SurfaceLifecycleEffectKind::SessionEnded { .. } => false,
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
    match layer.as_str() {
        "protocol" => serde_json::from_value::<ServerNotification>(payload)
            .map(CoreEvent::Protocol)
            .map_err(|error| {
                ClientError::InvalidArgument(format!("invalid protocol event: {error}"))
            }),
        "stream" => serde_json::from_value::<AgentStreamEvent>(payload)
            .map(CoreEvent::Stream)
            .map_err(|error| {
                ClientError::InvalidArgument(format!("invalid stream event: {error}"))
            }),
        "tui" => serde_json::from_value::<TuiOnlyEvent>(payload)
            .map(CoreEvent::Tui)
            .map_err(|error| ClientError::InvalidArgument(format!("invalid tui event: {error}"))),
        other => Err(ClientError::InvalidArgument(format!(
            "unknown session/event layer: {other}"
        ))),
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
use coco_types::AgentId;
use coco_types::AgentStreamEvent;
use coco_types::CoreEvent;
use coco_types::ServerNotification;
use coco_types::SessionEnvelope;
use coco_types::SessionId;
use coco_types::SessionSubscribeEnvelope;
use coco_types::SurfaceDelivery;
use coco_types::SurfaceId;
use coco_types::SurfaceLifecycleEffect;
use coco_types::SurfaceLifecycleEffectKind;
use coco_types::TuiOnlyEvent;
use tokio::sync::mpsc;

use super::ClientError;
use super::MAX_BUFFERED_CONNECTION_QUEUE;
