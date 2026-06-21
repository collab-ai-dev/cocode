//! Tier-3 error type for the provider-auth service (snafu + `coco-error`).

use coco_error::ErrorExt;
use coco_error::Location;
use coco_error::StatusCode;
use coco_error::stack_trace_debug;
use snafu::Snafu;

pub use provider_auth_error::*;

/// Errors raised while acquiring, refreshing, or storing provider credentials.
#[stack_trace_debug]
#[derive(Snafu)]
#[snafu(visibility(pub), module)]
pub enum ProviderAuthError {
    /// No credential is stored for this provider (the user must run `coco login`).
    #[snafu(display("not logged in for provider '{provider}': run `coco login {provider}`"))]
    NotLoggedIn {
        provider: String,
        #[snafu(implicit)]
        location: Location,
    },

    /// The refresh token is expired/reused/revoked — re-login required.
    #[snafu(display("session expired for provider '{provider}': run `coco login {provider}`"))]
    SessionExpired {
        provider: String,
        #[snafu(implicit)]
        location: Location,
    },

    /// The OAuth token endpoint returned an error (non-2xx).
    #[snafu(display("token endpoint error ({status}): {message}"))]
    TokenEndpoint {
        status: i32,
        message: String,
        #[snafu(implicit)]
        location: Location,
    },

    /// CSRF `state` mismatch on the callback — possible interception.
    #[snafu(display("OAuth state mismatch (possible CSRF) for provider '{provider}'"))]
    StateMismatch {
        provider: String,
        #[snafu(implicit)]
        location: Location,
    },

    /// The OAuth callback reported an error (e.g. `access_denied`).
    #[snafu(display("OAuth callback error: {message}"))]
    Callback {
        message: String,
        #[snafu(implicit)]
        location: Location,
    },

    /// Could not bind the loopback callback listener.
    #[snafu(display("could not bind loopback callback server: {message}"))]
    LoopbackBind {
        message: String,
        #[snafu(implicit)]
        location: Location,
    },

    /// Network/transport error talking to the auth server.
    #[snafu(display("network error: {message}"))]
    Network {
        message: String,
        #[snafu(implicit)]
        location: Location,
    },

    /// Credential store (keyring / file) I/O error.
    #[snafu(display("credential store error: {message}"))]
    Store {
        message: String,
        #[snafu(implicit)]
        location: Location,
    },

    /// Internal invariant violation (parse, encode, …).
    #[snafu(display("internal provider-auth error: {message}"))]
    Internal {
        message: String,
        #[snafu(implicit)]
        location: Location,
    },
}

impl ErrorExt for ProviderAuthError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::NotLoggedIn { .. } => StatusCode::InvalidConfig,
            Self::SessionExpired { .. } => StatusCode::AuthenticationFailed,
            Self::TokenEndpoint { .. } => StatusCode::ProviderError,
            Self::StateMismatch { .. } => StatusCode::AuthenticationFailed,
            Self::Callback { .. } => StatusCode::AuthenticationFailed,
            Self::LoopbackBind { .. } => StatusCode::IoError,
            Self::Network { .. } => StatusCode::NetworkError,
            Self::Store { .. } => StatusCode::IoError,
            Self::Internal { .. } => StatusCode::Internal,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub type Result<T> = std::result::Result<T, ProviderAuthError>;
