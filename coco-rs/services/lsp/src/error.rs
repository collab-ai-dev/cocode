//! LSP error types

use thiserror::Error;

pub type Result<T> = std::result::Result<T, LspErr>;

#[derive(Error, Debug)]
pub enum LspErr {
    /// Server binary not found (pre-installed requirement)
    #[error("LSP server not found: {server}. Install: {hint}")]
    ServerNotFound { server: String, hint: String },

    /// Server command not installed (command not in PATH)
    #[error(
        "LSP server '{server_id}' not installed: command '{command}' not found. Install: {install_hint}"
    )]
    ServerNotInstalled {
        server_id: String,
        command: String,
        install_hint: String,
    },

    /// Server failed to start
    #[error("failed to start LSP server {server}: {reason}")]
    ServerStartFailed { server: String, reason: String },

    /// Server initialization timeout (45s)
    #[error("LSP server initialization timed out after {timeout_secs}s")]
    InitializationTimeout { timeout_secs: i32 },

    /// Server failed after max restart attempts
    #[error("LSP server {server} failed after {restarts} restart attempts")]
    ServerFailed { server: String, restarts: i32 },

    /// Server is restarting, please retry
    #[error("LSP server {server} is restarting, please retry")]
    ServerRestarting { server: String },

    /// Health check failed
    #[error("LSP server {server} health check failed: {reason}")]
    HealthCheckFailed { server: String, reason: String },

    /// JSON-RPC protocol error
    #[error("JSON-RPC error in '{method}': {message}")]
    JsonRpc {
        method: String,
        message: String,
        code: Option<i32>,
    },

    /// No server available for file extension
    #[error("no LSP server available for file extension: {ext}")]
    NoServerForExtension { ext: String },

    /// Server config missing command and no builtin template found
    #[error("Server '{server_id}' missing command and no builtin template found. {hint}")]
    MissingCommand { server_id: String, hint: String },

    /// Server does not support the requested operation
    #[error("LSP server does not support {operation}")]
    OperationNotSupported { operation: String },

    /// Symbol not found in document
    #[error("symbol '{name}' not found in {file}")]
    SymbolNotFound { name: String, file: String },

    /// File not found or inaccessible
    #[error("file not found: {path}")]
    FileNotFound { path: String },

    /// Request timeout
    #[error("LSP request timed out after {timeout_secs}s")]
    RequestTimeout { timeout_secs: i32 },

    /// Connection closed unexpectedly
    #[error("LSP connection closed unexpectedly")]
    ConnectionClosed,

    /// Invalid UTF-8 in message
    #[error("invalid UTF-8 in LSP message: {0}")]
    InvalidUtf8(String),

    /// Internal error
    #[error("internal LSP error: {0}")]
    Internal(String),

    /// Configuration error
    #[error("LSP configuration error: {0}")]
    ConfigError(String),

    /// Installation error
    #[error("LSP server installation failed: {0}")]
    InstallError(String),

    /// IO error
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// JSON serialization error
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

// `LspErr` keeps its `thiserror` shape (callers throughout the LSP crate
// construct variants directly via `Self::Variant { .. }`); we layer the
// `coco-error` traits on top so callers can match on `StatusCode` for
// retry / classification without the mass-rewrite that a full snafu
// migration would require. The status mapping intentionally favors
// existing `coco_error::StatusCode` variants over introducing new ones.
impl coco_error::StackError for LspErr {
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>) {
        buf.push(format!("{layer}: {self}"));
    }

    fn next(&self) -> Option<&dyn coco_error::StackError> {
        None
    }
}

impl coco_error::ErrorExt for LspErr {
    fn status_code(&self) -> coco_error::StatusCode {
        use coco_error::StatusCode;
        match self {
            Self::ServerNotFound { .. } | Self::ServerNotInstalled { .. } => {
                StatusCode::ProviderNotFound
            }
            Self::ServerStartFailed { .. } | Self::ServerFailed { .. } => {
                StatusCode::ConnectionFailed
            }
            Self::InitializationTimeout { .. } | Self::RequestTimeout { .. } => StatusCode::Timeout,
            Self::ServerRestarting { .. } => StatusCode::ResourcesExhausted,
            Self::HealthCheckFailed { .. } | Self::ConnectionClosed => StatusCode::NetworkError,
            Self::JsonRpc { .. } => StatusCode::ProviderError,
            Self::NoServerForExtension { .. } | Self::OperationNotSupported { .. } => {
                StatusCode::Unsupported
            }
            Self::SymbolNotFound { .. } => StatusCode::InvalidArguments,
            Self::FileNotFound { .. } => StatusCode::FileNotFound,
            Self::MissingCommand { .. } | Self::ConfigError(_) => StatusCode::InvalidConfig,
            Self::InvalidUtf8(_) => StatusCode::ParseError,
            Self::InstallError(_) => StatusCode::External,
            Self::Internal(_) => StatusCode::Internal,
            Self::Io(_) => StatusCode::IoError,
            Self::Json(_) => StatusCode::InvalidJson,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
