//! Typed errors for resolution and validation.
//!
//! Used by `PartialProviderConfig::from_partial`,
//! `PartialModelInfo::from_partial`, `PositiveTokens::try_from`, and
//! `build_model_registry`. Surface-level callers convert into `anyhow`
//! at the runtime-config builder boundary.

use coco_error::ErrorExt;
use coco_error::StackError;
use coco_error::StatusCode;
use coco_types::ReasoningEffort;
use std::fmt;
use std::path::PathBuf;
use thiserror::Error;

/// Required field on a partial provider or model overlay. Closed set
/// so error reporting can branch / format without `&'static str`
/// churn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigField {
    // Provider-level
    Api,
    EnvKey,
    BaseUrl,
    // Model-level
    ContextWindow,
    MaxOutputTokens,
}

impl ConfigField {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::EnvKey => "env_key",
            Self::BaseUrl => "base_url",
            Self::ContextWindow => "context_window",
            Self::MaxOutputTokens => "max_output_tokens",
        }
    }
}

impl fmt::Display for ConfigField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error(
        "provider `{name}`: missing required field `{field}` (settings.json overlay must declare it for new providers)"
    )]
    IncompleteProviderEntry { name: String, field: ConfigField },

    #[error(
        "model `{provider}/{model}`: missing required field `{field}` — declare it in `models.json` or `providers.<name>.models.<id>`"
    )]
    IncompleteModelEntry {
        provider: String,
        model: String,
        field: ConfigField,
    },

    #[error(
        "non-positive token value {value} — `context_window` / `max_output_tokens` must be a positive integer that fits in u32"
    )]
    NonPositiveTokens { value: i64 },

    #[error(
        "non-positive count value {value} — `top_k` and similar fields must be a positive integer that fits in u32"
    )]
    NonPositiveCount { value: i64 },

    #[error("failed to read base_instructions_file at {path}: {source}")]
    BaseInstructionsRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to read {path}: {source}")]
    CatalogRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse {path}: {message}")]
    CatalogParse { path: PathBuf, message: String },

    #[error("jsonc error: {message}")]
    Jsonc { message: String },

    #[error("unknown provider `{name}` referenced by role binding")]
    UnknownProvider { name: String },

    #[error(
        "unknown model `{provider}/{model}` — not in builtin registry, models.json, or per-provider models"
    )]
    UnknownModel { provider: String, model: String },

    #[error(
        "provider `{name}`: invalid timeout_secs {value} — must be >= 0 (use 0 to disable per-request timeout)"
    )]
    InvalidTimeoutSecs { name: String, value: i64 },

    #[error(
        "model `{provider}/{model}`: default_thinking_level `{default}` is not declared in supported_thinking_levels (declared: {supported:?})"
    )]
    DefaultThinkingLevelNotSupported {
        provider: String,
        model: String,
        default: ReasoningEffort,
        supported: Vec<ReasoningEffort>,
    },

    #[error(
        "model `{provider}/{model}`: max_output_tokens_escalate {escalate} must be strictly greater than max_output_tokens {baseline} — set it higher to enable Phase-1 escalate, or omit it to disable escalation for this model"
    )]
    EscalateBelowBaseline {
        provider: String,
        model: String,
        baseline: i64,
        escalate: i64,
    },

    #[error("{message}")]
    InvalidConfig { message: String },

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

    #[error("{message}")]
    Generic { message: String },
}

impl ConfigError {
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
}

/// Anyhow-style `.context()` adapter for non-`anyhow::Result` paths.
pub trait ResultExt<T> {
    fn with_ctx(self, msg: impl Into<String>) -> Result<T, ConfigError>;
    fn with_ctx_lazy<F: FnOnce() -> String>(self, msg: F) -> Result<T, ConfigError>;
}

impl<T, E: std::fmt::Display> ResultExt<T> for Result<T, E> {
    fn with_ctx(self, msg: impl Into<String>) -> Result<T, ConfigError> {
        self.map_err(|e| ConfigError::Generic {
            message: format!("{}: {e}", msg.into()),
        })
    }

    fn with_ctx_lazy<F: FnOnce() -> String>(self, msg: F) -> Result<T, ConfigError> {
        self.map_err(|e| ConfigError::Generic {
            message: format!("{}: {e}", msg()),
        })
    }
}

pub type Result<T, E = ConfigError> = std::result::Result<T, E>;

impl StackError for ConfigError {
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>) {
        buf.push(format!("{layer}: {self}"));
    }

    fn next(&self) -> Option<&dyn StackError> {
        None
    }
}

impl ErrorExt for ConfigError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::IncompleteProviderEntry { .. } | Self::IncompleteModelEntry { .. } => {
                StatusCode::InvalidConfig
            }
            Self::NonPositiveTokens { .. } | Self::NonPositiveCount { .. } => {
                StatusCode::InvalidArguments
            }
            Self::BaseInstructionsRead { .. } | Self::CatalogRead { .. } => StatusCode::IoError,
            Self::CatalogParse { .. } => StatusCode::ParseError,
            Self::UnknownProvider { .. } => StatusCode::ProviderNotFound,
            Self::UnknownModel { .. } => StatusCode::ModelNotFound,
            Self::InvalidTimeoutSecs { .. } => StatusCode::InvalidArguments,
            Self::DefaultThinkingLevelNotSupported { .. }
            | Self::EscalateBelowBaseline { .. }
            | Self::InvalidConfig { .. } => StatusCode::InvalidConfig,
            Self::Io { .. } => StatusCode::IoError,
            Self::Json { .. } => StatusCode::InvalidJson,
            Self::Jsonc { .. } => StatusCode::InvalidJson,
            Self::Generic { .. } => StatusCode::Internal,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
