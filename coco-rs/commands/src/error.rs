//! Typed error for the coco-commands crate.
//!
//! Tier 3 main-trunk: implements `coco_error::ErrorExt` so callers can
//! classify failures by `StatusCode`.

use coco_error::ErrorExt;
use coco_error::StackError;
use coco_error::StatusCode;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CommandsError {
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

    #[error("git failed: {message}")]
    GitFailed { message: String },

    /// In-prompt shell expansion was aborted because a command was
    /// permission-denied or failed, with no partial substitution.
    #[error("shell command failed: {message}")]
    ShellCommandError { message: String },

    #[error("unknown command: /{name}")]
    UnknownCommand { name: String },

    #[error("command /{name} not available: {reason}")]
    CommandUnavailable { name: String, reason: String },

    #[error("plugin error: {source}")]
    Plugin {
        #[from]
        source: coco_plugins::PluginError,
    },

    #[error("task join error: {source}")]
    Join {
        #[from]
        source: tokio::task::JoinError,
    },
}

impl CommandsError {
    pub fn generic(message: impl Into<String>) -> Self {
        Self::Generic {
            message: message.into(),
        }
    }

    pub fn git_failed(message: impl Into<String>) -> Self {
        Self::GitFailed {
            message: message.into(),
        }
    }
}

impl StackError for CommandsError {
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>) {
        buf.push(format!("{layer}: {self}"));
    }

    fn next(&self) -> Option<&dyn StackError> {
        None
    }
}

impl ErrorExt for CommandsError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::Io { .. } => StatusCode::IoError,
            Self::Json { .. } => StatusCode::InvalidJson,
            Self::GitFailed { .. } => StatusCode::External,
            Self::ShellCommandError { .. } => StatusCode::External,
            Self::UnknownCommand { .. } | Self::CommandUnavailable { .. } => {
                StatusCode::InvalidArguments
            }
            Self::Plugin { source } => source.status_code(),
            Self::Join { .. } => StatusCode::Internal,
            Self::Generic { .. } => StatusCode::Internal,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub type Result<T, E = CommandsError> = std::result::Result<T, E>;
