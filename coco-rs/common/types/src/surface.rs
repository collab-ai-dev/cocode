use crate::{RequestId, ServerRequest, SessionEnvelope, SessionId, SurfaceId};

/// One outbound session event targeted to a client surface.
#[derive(Debug, Clone)]
pub struct SurfaceDelivery {
    pub surface_id: SurfaceId,
    pub envelope: SessionEnvelope,
}

/// One actionable server request targeted to a client surface.
#[derive(Debug, Clone)]
pub struct ServerRequestDelivery {
    pub session_id: SessionId,
    pub surface_id: SurfaceId,
    pub request_id: RequestId,
    pub request: ServerRequest,
}

/// One session lifecycle transition targeted to a client surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceLifecycleEffect {
    pub surface_id: SurfaceId,
    pub kind: SurfaceLifecycleEffectKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceLifecycleEffectKind {
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
