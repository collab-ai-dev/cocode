//! Typed error for the coco-hooks crate.
//!
//! Tier 3 main-trunk: implements `coco_error::ErrorExt` so hook
//! orchestration failures classify correctly at the boundary.

use coco_error::ErrorExt;
use coco_error::StackError;
use coco_error::StatusCode;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HooksError {
    #[error("{message}")]
    Generic { message: String },

    #[error("io error: {source}")]
    Io {
        #[from]
        source: std::io::Error,
    },

    #[error("json error: {source}")]
    Json {
        #[from]
        source: serde_json::Error,
    },

    #[error("hook command timed out after {timeout_ms}ms")]
    HookTimeout { timeout_ms: u64 },

    #[error("hook command failed: {message}")]
    HookExecFailed { message: String },

    #[error("hook config invalid: {message}")]
    InvalidConfig { message: String },

    #[error("HTTP hook request failed: {message}")]
    HttpFailed { message: String },

    #[error("SSRF check failed for {url}: {message}")]
    SsrfFailed { url: String, message: String },
}

impl HooksError {
    pub fn generic(message: impl Into<String>) -> Self {
        Self::Generic {
            message: message.into(),
        }
    }

    pub fn invalid_config(message: impl Into<String>) -> Self {
        Self::InvalidConfig {
            message: message.into(),
        }
    }

    pub fn exec_failed(message: impl Into<String>) -> Self {
        Self::HookExecFailed {
            message: message.into(),
        }
    }
}

impl StackError for HooksError {
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>) {
        buf.push(format!("{layer}: {self}"));
    }

    fn next(&self) -> Option<&dyn StackError> {
        None
    }
}

impl ErrorExt for HooksError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::Io { .. } => StatusCode::IoError,
            Self::Json { .. } => StatusCode::InvalidJson,
            Self::HookTimeout { .. } => StatusCode::Timeout,
            Self::HookExecFailed { .. } => StatusCode::External,
            Self::InvalidConfig { .. } => StatusCode::InvalidConfig,
            Self::HttpFailed { .. } => StatusCode::NetworkError,
            Self::SsrfFailed { .. } => StatusCode::PermissionDenied,
            Self::Generic { .. } => StatusCode::Internal,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub type Result<T, E = HooksError> = std::result::Result<T, E>;
