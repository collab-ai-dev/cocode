use crate::{RequestId, ServerRequest, SessionEnvelope, SessionId};

/// Access granted to one connection for a session.
///
/// Authorization is independent of the live event attachment: close may
/// preserve a grant for durable reads or deletion while dropping delivery.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionAccess {
    ReadOnly,
    Full,
}

/// One outbound session event targeted to a client connection.
#[derive(Debug, Clone)]
pub struct SessionDelivery {
    pub envelope: SessionEnvelope,
}

/// One actionable server request targeted to a full-access client.
#[derive(Debug, Clone)]
pub struct ServerRequestDelivery {
    pub session_id: SessionId,
    pub request_id: RequestId,
    pub request: ServerRequest,
}

/// One session lifecycle transition targeted to a client connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLifecycleEffect {
    pub kind: SessionLifecycleEffectKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionLifecycleEffectKind {
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
