//! Typed error for the rmcp-client crate.
//!
//! Tier 3 main-trunk: implements `coco_error::ErrorExt` so callers can
//! classify failures by `StatusCode` without losing the underlying source.

use coco_error::ErrorExt;
use coco_error::StackError;
use coco_error::StatusCode;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RmcpClientError {
    #[error("{message}")]
    Generic { message: String },

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("keyring error: {0}")]
    Keyring(#[from] keyring::Error),

    #[error("rmcp service error: {0}")]
    Service(#[from] rmcp::service::ServiceError),

    #[error("rmcp auth error: {0}")]
    Auth(#[from] rmcp::transport::auth::AuthError),

    #[error("OAuth error: {message}")]
    OAuth { message: String },

    #[error("MCP client {state}")]
    InvalidState { state: &'static str },
}

impl RmcpClientError {
    pub fn generic(message: impl Into<String>) -> Self {
        Self::Generic {
            message: message.into(),
        }
    }

    pub fn oauth(message: impl Into<String>) -> Self {
        Self::OAuth {
            message: message.into(),
        }
    }
}

/// Anyhow-style `.context(msg)` adapter for `Result<T, E>` where `E` already
/// implements `Display`. Wraps the source error message into a `Generic`
/// variant prefixed with the supplied context.
pub trait ResultExt<T> {
    fn with_ctx(self, msg: impl Into<String>) -> Result<T, RmcpClientError>;
    fn with_ctx_lazy<F: FnOnce() -> String>(self, msg: F) -> Result<T, RmcpClientError>;
}

impl<T, E: std::fmt::Display> ResultExt<T> for Result<T, E> {
    fn with_ctx(self, msg: impl Into<String>) -> Result<T, RmcpClientError> {
        self.map_err(|e| RmcpClientError::Generic {
            message: format!("{}: {e}", msg.into()),
        })
    }

    fn with_ctx_lazy<F: FnOnce() -> String>(self, msg: F) -> Result<T, RmcpClientError> {
        self.map_err(|e| RmcpClientError::Generic {
            message: format!("{}: {e}", msg()),
        })
    }
}

impl StackError for RmcpClientError {
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>) {
        buf.push(format!("{layer}: {self}"));
    }

    fn next(&self) -> Option<&dyn StackError> {
        None
    }
}

impl ErrorExt for RmcpClientError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::Io(_) => StatusCode::IoError,
            Self::Json(_) => StatusCode::InvalidJson,
            Self::Http(_) => StatusCode::NetworkError,
            Self::Keyring(_) => StatusCode::IoError,
            Self::Service(_) => StatusCode::External,
            Self::Auth(_) => StatusCode::AuthenticationFailed,
            Self::OAuth { .. } => StatusCode::AuthenticationFailed,
            Self::InvalidState { .. } => StatusCode::InvalidConfig,
            Self::Generic { .. } => StatusCode::Internal,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub type Result<T, E = RmcpClientError> = std::result::Result<T, E>;
