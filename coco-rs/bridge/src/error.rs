//! Crate-local error type for the IDE / REPL bridge protocols. Tier 2
//! leaf-lib status forbids depending on `coco-error`, so this is a pure
//! `thiserror` enum; main-trunk callers convert at the boundary.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum BridgeError {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error("outgoing channel closed")]
    ChannelClosed,

    #[error("no IDE subscribers")]
    NoSubscribers,

    #[error("{message}")]
    Generic { message: String },
}

impl BridgeError {
    pub fn generic(message: impl Into<String>) -> Self {
        Self::Generic {
            message: message.into(),
        }
    }
}
