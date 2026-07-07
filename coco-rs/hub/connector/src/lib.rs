use chrono::DateTime;
use chrono::Utc;
use coco_hub_protocol::BatchFrame;
use coco_hub_protocol::EventEnvelope;
use coco_hub_protocol::EventPayload;
use coco_hub_protocol::SCHEMA_VERSION_V2;
use coco_types::CoreEvent;
use coco_types::SessionEnvelope;
use uuid::Uuid;

pub use coco_hub_protocol as protocol;

#[derive(Debug, thiserror::Error)]
pub enum EnvelopeEgressError {
    #[error("durable hub egress only accepts protocol-layer envelopes")]
    NonProtocolDurable,
    #[error("failed to serialize protocol notification: {0}")]
    Serialize(#[from] serde_json::Error),
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
mod tests {
    use chrono::TimeZone;
    use coco_types::AgentId;
    use coco_types::AgentStreamEvent;
    use coco_types::CoreEvent;
    use coco_types::EventReplayPolicy;
    use coco_types::ServerNotification;
    use coco_types::SessionEnvelope;
    use coco_types::SessionId;
    use coco_types::SessionStartedParams;
    use serde_json::json;
    use uuid::Uuid;

    use super::*;

    fn session_id() -> SessionId {
        SessionId::try_new("session-1").expect("valid session id")
    }

    fn agent_id() -> AgentId {
        AgentId::try_new_generated("aagent-0000000000000001").expect("valid generated agent id")
    }

    fn fixed_ts() -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(1_704_067_200, 0)
            .single()
            .expect("fixed timestamp")
    }

    fn session_started(session_id: SessionId) -> ServerNotification {
        ServerNotification::SessionStarted(SessionStartedParams {
            session_id,
            protocol_version: "1.0".into(),
            cwd: "/work".into(),
            model: "claude".into(),
            provider: "anthropic".into(),
            permission_mode: "default".into(),
            tools: vec!["Read".into()],
            slash_commands: Vec::new(),
            agents: Vec::new(),
            skills: Vec::new(),
            mcp_servers: Vec::new(),
            plugins: Vec::new(),
            api_key_source: None,
            betas: Vec::new(),
            version: "0.1.0".into(),
            output_style: None,
            fast_mode_state: None,
            lsp_active: false,
        })
    }

    #[test]
    fn durable_protocol_envelope_becomes_hub_event_envelope() {
        let session_id = session_id();
        let agent_id = agent_id();
        let envelope = SessionEnvelope::durable(
            session_id.clone(),
            Some(agent_id.clone()),
            None,
            42,
            CoreEvent::Protocol(session_started(session_id.clone())),
        );

        let event =
            event_envelope_from_session_envelope(Uuid::nil(), fixed_ts(), envelope).unwrap();
        let event = event.expect("durable envelope should be shipped");

        assert_eq!(event.instance_id, Uuid::nil());
        assert_eq!(event.session_id, session_id);
        assert_eq!(event.agent_id, Some(agent_id));
        assert_eq!(event.session_seq, 42);
        assert_eq!(event.schema_version, SCHEMA_VERSION_V2);

        let EventPayload::Protocol { value } = event.payload else {
            panic!("expected protocol payload");
        };
        assert_eq!(value["method"], "session/started");
        assert_eq!(value["params"]["session_id"], "session-1");
    }

    #[test]
    fn batch_conversion_preserves_durable_order_and_skips_ephemeral() {
        let first_session = session_id();
        let second_session = SessionId::try_new("session-2").expect("valid session id");
        let envelopes = vec![
            SessionEnvelope::durable(
                first_session.clone(),
                None,
                None,
                1,
                CoreEvent::Protocol(session_started(first_session.clone())),
            ),
            SessionEnvelope::ephemeral(
                first_session.clone(),
                None,
                None,
                CoreEvent::Stream(AgentStreamEvent::ToolUseQueued {
                    call_id: "call-1".into(),
                    name: "Read".into(),
                    input: json!({}),
                }),
            ),
            SessionEnvelope::durable(
                second_session.clone(),
                None,
                None,
                7,
                CoreEvent::Protocol(session_started(second_session.clone())),
            ),
        ];

        let batch = batch_frame_from_session_envelopes(Uuid::nil(), fixed_ts, envelopes).unwrap();

        assert_eq!(batch.events.len(), 2);
        assert_eq!(batch.events[0].session_id, first_session);
        assert_eq!(batch.events[0].session_seq, 1);
        assert_eq!(batch.events[1].session_id, second_session);
        assert_eq!(batch.events[1].session_seq, 7);
    }

    #[test]
    fn ephemeral_envelope_is_not_shipped_to_hub() {
        let envelope = SessionEnvelope::stamp(
            session_id(),
            None,
            CoreEvent::Stream(AgentStreamEvent::ToolUseQueued {
                call_id: "call-1".into(),
                name: "Read".into(),
                input: json!({"file_path": "a.txt"}),
            }),
            || panic!("ephemeral events must not allocate session_seq"),
        );

        assert_eq!(envelope.event.replay_policy(), EventReplayPolicy::Ephemeral);
        let event =
            event_envelope_from_session_envelope(Uuid::nil(), fixed_ts(), envelope).unwrap();
        assert!(event.is_none());
    }

    #[test]
    fn sequenced_non_protocol_envelope_is_rejected() {
        let envelope = SessionEnvelope::durable(
            session_id(),
            None,
            None,
            1,
            CoreEvent::Stream(AgentStreamEvent::ToolUseQueued {
                call_id: "call-1".into(),
                name: "Read".into(),
                input: json!({}),
            }),
        );

        let err = event_envelope_from_session_envelope(Uuid::nil(), fixed_ts(), envelope)
            .expect_err("sequenced stream event must be rejected");
        assert!(matches!(err, EnvelopeEgressError::NonProtocolDurable));
    }
}
