use std::collections::HashMap;

use coco_app_server_transport::{
    JsonRpcFrame, JsonRpcNotification as TransportJsonRpcNotification,
};
use coco_types::{
    AgentId, AgentStreamEvent, ServerNotification, SessionId, StreamAccumulator, SurfaceId, TurnId,
};
#[cfg(test)]
use coco_types::{CoreEvent, JSONRPC_VERSION, JsonRpcNotification as LegacyJsonRpcNotification};
use serde::Deserialize;
#[cfg(test)]
use serde_json::Value;
use tracing::warn;

#[derive(Default)]
pub(crate) struct SdkEventRenderer {
    accumulators: HashMap<SessionId, StreamAccumulator>,
    pre_turn: HashMap<SessionId, Vec<AgentStreamEvent>>,
}

impl SdkEventRenderer {
    const PRE_TURN_BUFFER_CAP: usize = 64;

    pub(crate) fn render_frame(
        &mut self,
        frame: JsonRpcFrame,
    ) -> Result<Vec<JsonRpcFrame>, (serde_json::Error, JsonRpcFrame)> {
        let JsonRpcFrame::Notification(notification) = &frame else {
            return Ok(vec![frame]);
        };
        if notification.method != coco_types::SESSION_EVENT_METHOD {
            return Ok(vec![frame]);
        }
        match self.render_session_event(notification) {
            Ok(frames) => Ok(frames),
            Err(error) => Err((error, frame)),
        }
    }

    fn render_session_event(
        &mut self,
        notification: &TransportJsonRpcNotification,
    ) -> Result<Vec<JsonRpcFrame>, serde_json::Error> {
        let routed: RoutedSdkEvent = serde_json::from_value(
            notification
                .params
                .clone()
                .unwrap_or(serde_json::Value::Null),
        )?;
        let metadata = RoutedEventMetadata {
            surface_id: routed.surface_id,
            session_id: routed.envelope.session_id.clone(),
            agent_id: routed.envelope.agent_id,
            turn_id: routed.envelope.turn_id,
            session_seq: routed.envelope.session_seq,
        };
        let layer = routed
            .envelope
            .event
            .layer
            .parse::<coco_types::EventLayer>()
            .map_err(|()| {
                <serde_json::Error as serde::de::Error>::custom(format!(
                    "unknown routed CoreEvent layer: {}",
                    routed.envelope.event.layer
                ))
            })?;
        let notifications = match layer {
            coco_types::EventLayer::Protocol => {
                let notification: ServerNotification =
                    serde_json::from_value(routed.envelope.event.payload)?;
                self.render_protocol_event(&routed.envelope.session_id, notification)
            }
            coco_types::EventLayer::Stream => {
                let event: AgentStreamEvent =
                    serde_json::from_value(routed.envelope.event.payload)?;
                self.render_stream_event(&routed.envelope.session_id, event)
            }
            coco_types::EventLayer::Tui => Vec::new(),
        };
        notifications
            .into_iter()
            .map(|notification| notification_frame(notification, &metadata))
            .collect()
    }

    fn render_protocol_event(
        &mut self,
        session_id: &SessionId,
        notification: ServerNotification,
    ) -> Vec<ServerNotification> {
        let mut rendered = Vec::new();
        match &notification {
            ServerNotification::TurnStarted(params) => {
                let mut accumulator = StreamAccumulator::new(params.turn_id.as_str().to_string());
                if let Some(buffered) = self.pre_turn.remove(session_id) {
                    rendered.extend(
                        buffered
                            .into_iter()
                            .flat_map(|event| accumulator.process(event)),
                    );
                }
                self.accumulators.insert(session_id.clone(), accumulator);
            }
            ServerNotification::TurnEnded(_) => {
                if let Some(mut accumulator) = self.accumulators.remove(session_id) {
                    rendered.extend(accumulator.flush());
                }
                self.pre_turn.remove(session_id);
            }
            _ => {}
        }
        rendered.push(notification);
        rendered
    }

    fn render_stream_event(
        &mut self,
        session_id: &SessionId,
        event: AgentStreamEvent,
    ) -> Vec<ServerNotification> {
        if let Some(accumulator) = self.accumulators.get_mut(session_id) {
            return accumulator.process(event);
        }
        let buffer = self.pre_turn.entry(session_id.clone()).or_default();
        if buffer.len() >= Self::PRE_TURN_BUFFER_CAP {
            warn!(
                session_id = %session_id,
                cap = Self::PRE_TURN_BUFFER_CAP,
                "pre-turn SDK stream buffer full; dropping event"
            );
        } else {
            buffer.push(event);
        }
        Vec::new()
    }
}

#[derive(Deserialize)]
struct RoutedSdkEvent {
    surface_id: SurfaceId,
    envelope: RoutedSdkEnvelope,
}

#[derive(Deserialize)]
struct RoutedSdkEnvelope {
    session_id: SessionId,
    #[serde(default)]
    agent_id: Option<AgentId>,
    #[serde(default)]
    turn_id: Option<TurnId>,
    #[serde(default)]
    session_seq: Option<i64>,
    event: RoutedCoreEvent,
}

#[derive(Deserialize)]
struct RoutedCoreEvent {
    layer: String,
    payload: serde_json::Value,
}

struct RoutedEventMetadata {
    surface_id: SurfaceId,
    session_id: SessionId,
    agent_id: Option<AgentId>,
    turn_id: Option<TurnId>,
    session_seq: Option<i64>,
}

fn notification_frame(
    notification: ServerNotification,
    metadata: &RoutedEventMetadata,
) -> Result<JsonRpcFrame, serde_json::Error> {
    let serde_json::Value::Object(mut value) = serde_json::to_value(notification)? else {
        unreachable!("ServerNotification always serializes as an object");
    };
    let method = match value.remove("method") {
        Some(serde_json::Value::String(method)) => method,
        _ => unreachable!("ServerNotification always serializes a string method"),
    };
    let mut params = match value.remove("params") {
        Some(serde_json::Value::Object(params)) => params,
        Some(serde_json::Value::Null) | None => serde_json::Map::new(),
        Some(other) => {
            let mut params = serde_json::Map::new();
            params.insert("payload".to_string(), other);
            params
        }
    };
    params.insert(
        "surface_id".to_string(),
        serde_json::to_value(&metadata.surface_id)?,
    );
    params.insert(
        "session_id".to_string(),
        serde_json::to_value(&metadata.session_id)?,
    );
    params.insert(
        "agent_id".to_string(),
        serde_json::to_value(&metadata.agent_id)?,
    );
    params.insert(
        "turn_id".to_string(),
        serde_json::to_value(&metadata.turn_id)?,
    );
    params.insert(
        "session_seq".to_string(),
        serde_json::to_value(metadata.session_seq)?,
    );
    Ok(JsonRpcFrame::Notification(
        TransportJsonRpcNotification::new(method, Some(serde_json::Value::Object(params))),
    ))
}

/// Translate a `CoreEvent` into its legacy direct-notification view.
/// Production emits canonical `session/event` envelopes; this helper remains
/// only for focused serialization tests.
#[cfg(test)]
pub(crate) fn core_event_to_notification(event: CoreEvent) -> Option<LegacyJsonRpcNotification> {
    match event {
        CoreEvent::Protocol(notif) => server_notification_to_jsonrpc(notif),
        CoreEvent::Stream(_) => None,
        CoreEvent::Tui(_) => None,
    }
}

/// Serialize a `ServerNotification` as a legacy direct `JsonRpcNotification`.
/// Production emits canonical `session/event` frames; this remains only for
/// focused serialization tests.
#[cfg(test)]
fn server_notification_to_jsonrpc(notif: ServerNotification) -> Option<LegacyJsonRpcNotification> {
    match serde_json::to_value(notif).ok()? {
        Value::Object(mut map) => {
            let method = match map.remove("method")? {
                Value::String(s) => s,
                _ => return None,
            };
            let params = map.remove("params").unwrap_or(Value::Null);
            Some(LegacyJsonRpcNotification {
                jsonrpc: JSONRPC_VERSION.into(),
                method,
                params,
            })
        }
        _ => None,
    }
}
