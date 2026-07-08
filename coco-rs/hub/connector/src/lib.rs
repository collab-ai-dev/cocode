use chrono::DateTime;
use chrono::Utc;
use coco_hub_protocol::AnnounceAckFrame;
use coco_hub_protocol::AnnounceFrame;
use coco_hub_protocol::BatchAckFrame;
use coco_hub_protocol::BatchFrame;
use coco_hub_protocol::ErrorFrame;
use coco_hub_protocol::EventEnvelope;
use coco_hub_protocol::EventPayload;
use coco_hub_protocol::HubFrame;
use coco_hub_protocol::SCHEMA_VERSION_V2;
use coco_hub_protocol::SUBPROTOCOL_V2;
use coco_types::CoreEvent;
use coco_types::SessionEnvelope;
use coco_utils_rustls_provider::ensure_rustls_crypto_provider;
use futures::SinkExt;
use futures::StreamExt;
use tokio::net::TcpStream;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use uuid::Uuid;

mod worker;

pub use worker::HubConnectorQueueError;
pub use worker::HubConnectorSender;
pub use worker::HubConnectorWorker;
pub use worker::HubConnectorWorkerConfig;
pub use worker::HubConnectorWorkerError;
pub use worker::HubConnectorWorkerStats;

pub use coco_hub_protocol as protocol;

type HubWebSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Debug, thiserror::Error)]
pub enum EnvelopeEgressError {
    #[error("durable hub egress only accepts protocol-layer envelopes")]
    NonProtocolDurable,
    #[error("failed to serialize protocol notification: {0}")]
    Serialize(#[from] serde_json::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum HubConnectorError {
    #[error("failed to build hub websocket request: {0}")]
    BuildRequest(#[source] tokio_tungstenite::tungstenite::Error),
    #[error("hub websocket connect failed: {0}")]
    Connect(#[source] tokio_tungstenite::tungstenite::Error),
    #[error("hub websocket send failed: {0}")]
    Send(#[source] tokio_tungstenite::tungstenite::Error),
    #[error("hub websocket receive failed: {0}")]
    Receive(#[source] tokio_tungstenite::tungstenite::Error),
    #[error("failed to serialize hub frame: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("hub rejected frame: {code}: {detail}")]
    HubError { code: String, detail: String },
    #[error("hub protocol error: {0}")]
    Protocol(String),
    #[error("hub returned unexpected frame: {0}")]
    UnexpectedFrame(&'static str),
    #[error("hub websocket closed")]
    Closed,
}

pub struct HubConnectorClient {
    stream: HubWebSocket,
}

impl HubConnectorClient {
    pub async fn connect(url: &str) -> Result<Self, HubConnectorError> {
        ensure_rustls_crypto_provider();
        let mut request = url
            .into_client_request()
            .map_err(HubConnectorError::BuildRequest)?;
        request.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            http::HeaderValue::from_static(SUBPROTOCOL_V2),
        );
        let (stream, response) = connect_async(request)
            .await
            .map_err(HubConnectorError::Connect)?;
        let selected_protocol = response
            .headers()
            .get("Sec-WebSocket-Protocol")
            .and_then(|value| value.to_str().ok());
        if selected_protocol != Some(SUBPROTOCOL_V2) {
            return Err(HubConnectorError::Protocol(format!(
                "hub did not select websocket subprotocol {SUBPROTOCOL_V2}"
            )));
        }
        Ok(Self { stream })
    }

    pub async fn announce(
        &mut self,
        announce: AnnounceFrame,
    ) -> Result<AnnounceAckFrame, HubConnectorError> {
        self.send_frame(HubFrame::Announce(announce)).await?;
        match self.recv_frame().await? {
            HubFrame::AnnounceAck(ack) => Ok(ack),
            HubFrame::Error(error) => Err(hub_error(error)),
            _ => Err(HubConnectorError::UnexpectedFrame("announce_ack")),
        }
    }

    pub async fn send_batch(
        &mut self,
        batch: BatchFrame,
    ) -> Result<BatchAckFrame, HubConnectorError> {
        self.send_frame(HubFrame::Batch(batch)).await?;
        match self.recv_frame().await? {
            HubFrame::BatchAck(ack) => Ok(ack),
            HubFrame::Error(error) => Err(hub_error(error)),
            _ => Err(HubConnectorError::UnexpectedFrame("batch_ack")),
        }
    }

    async fn send_frame(&mut self, frame: HubFrame) -> Result<(), HubConnectorError> {
        let text = serde_json::to_string(&frame)?;
        self.stream
            .send(Message::Text(text.into()))
            .await
            .map_err(HubConnectorError::Send)
    }

    async fn recv_frame(&mut self) -> Result<HubFrame, HubConnectorError> {
        loop {
            let Some(message) = self.stream.next().await else {
                return Err(HubConnectorError::Closed);
            };
            match message.map_err(HubConnectorError::Receive)? {
                Message::Text(text) => return Ok(serde_json::from_str(&text)?),
                Message::Binary(_) => {
                    return Err(HubConnectorError::Protocol(
                        "hub returned binary frame".to_string(),
                    ));
                }
                Message::Close(_) => return Err(HubConnectorError::Closed),
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
            }
        }
    }
}

fn hub_error(error: ErrorFrame) -> HubConnectorError {
    HubConnectorError::HubError {
        code: error.code,
        detail: error.detail,
    }
}

/// Convert an AppServer-stamped session envelope into the Hub v2 wire envelope.
///
/// Ephemeral envelopes have no `session_seq`, are live-only by contract, and are
/// therefore skipped. A sequenced non-protocol event indicates the stamping seam
/// violated the durable/ephemeral taxonomy and is treated as an error.
pub fn event_envelope_from_session_envelope(
    instance_id: Uuid,
    ts: DateTime<Utc>,
    envelope: SessionEnvelope,
) -> Result<Option<EventEnvelope>, EnvelopeEgressError> {
    let Some(session_seq) = envelope.session_seq else {
        return Ok(None);
    };

    let payload = match envelope.event {
        CoreEvent::Protocol(notification) => EventPayload::Protocol {
            value: serde_json::to_value(notification)?,
        },
        CoreEvent::Stream(_) | CoreEvent::Tui(_) => {
            return Err(EnvelopeEgressError::NonProtocolDurable);
        }
    };

    Ok(Some(EventEnvelope {
        instance_id,
        session_id: envelope.session_id,
        agent_id: envelope.agent_id,
        session_seq,
        ts,
        schema_version: SCHEMA_VERSION_V2,
        payload,
    }))
}

pub fn batch_frame_from_session_envelopes(
    instance_id: Uuid,
    mut next_ts: impl FnMut() -> DateTime<Utc>,
    envelopes: impl IntoIterator<Item = SessionEnvelope>,
) -> Result<BatchFrame, EnvelopeEgressError> {
    let mut events = Vec::new();
    for envelope in envelopes {
        if let Some(event) = event_envelope_from_session_envelope(instance_id, next_ts(), envelope)?
        {
            events.push(event);
        }
    }
    Ok(BatchFrame { events })
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
