use crate::RequestId;
use crate::ServerRequest;
use crate::SessionEnvelope;
use crate::SessionId;
use crate::SurfaceId;

/// One outbound session event targeted to a client surface.
#[derive(Debug, Clone)]
pub struct SurfaceDelivery {
    pub surface_id: SurfaceId,
    pub envelope: SessionEnvelope,
}

/// One actionable server request targeted to a client surface.
#[derive(Debug, Clone)]
pub struct ServerRequestDelivery {
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
