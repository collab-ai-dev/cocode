//! Per-method AppServer request handlers shared by local and remote adapters.
//!
//! Transport-specific correlation and outbound writing remain in connection
//! adapters; these handlers operate on a validated host request
//! context.

use tokio_util::sync::CancellationToken;

pub(crate) mod config;
pub(crate) mod goal;
mod initialize_metadata;
pub(crate) mod mcp;
pub(crate) mod rewind;
pub(crate) mod runtime;
pub(crate) mod session;
pub(crate) mod turn;
mod turn_shortcuts;

pub use crate::app_server_host::{HandlerContext, HandlerResult};
use crate::session_runtime::ActiveTurnHandles;

/// The AppServer protocol version coco-rs speaks.
pub const APP_SERVER_PROTOCOL_VERSION: &str = "1.0";

/// Default model id reported by `initialize` and used when `session/start` /
/// `setModel` omit a model param.
pub const DEFAULT_APP_SERVER_MODEL: &str = "claude-opus-4-6";

/// Default fast-mode / secondary model id advertised by `initialize`.
pub const DEFAULT_APP_SERVER_FAST_MODEL: &str = "claude-sonnet-4-6";

pub(crate) struct ActiveTurnStartState {
    pub session_id: coco_types::SessionId,
    pub turn_id: coco_types::TurnId,
    pub cancel_token: CancellationToken,
}

pub(crate) enum ActiveTurnStartError {
    NoActiveSession,
    TurnAlreadyRunning,
}

pub(crate) struct ShortcutTurnState {
    pub session_id: coco_types::SessionId,
    pub turn_id: coco_types::TurnId,
    pub session: crate::session_runtime::SessionHandle,
    /// Held for its lifetime: releases the coordinator turn-slot reservation
    /// back to `Idle` when this shortcut state is dropped.
    pub _reservation: crate::session_runtime::ShortcutReservationGuard,
}
