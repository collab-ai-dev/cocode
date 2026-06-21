//! Portable auth-status vocabulary, shared across `coco-provider-auth`
//! (producer), `app/cli` status / `app/tui` model picker, and the
//! `coco-inference` availability gates (consumers). A curated subset of
//! jcode's `jcode-auth-types` enums.
//!
//! These describe *how a provider authenticates and how ready it is* without
//! any credential material — a bool "logged in" cannot express a provider that
//! is authenticated but still needs a provider-specific onboarding step before
//! it can serve requests (e.g. Gemini Code Assist project resolution), which is
//! exactly `AuthReadinessLevel::Authenticated` vs `RequestValid`.

use serde::Deserialize;
use serde::Serialize;

/// Whether a usable credential is currently present.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthState {
    /// A non-expired credential is available.
    Available,
    /// A credential exists but is expired (and could not auto-refresh).
    Expired,
    /// No credential configured for this provider.
    #[default]
    NotConfigured,
}

/// How a provider's credential can be refreshed.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthRefreshSupport {
    #[default]
    Unknown,
    /// Refreshed automatically out-of-band (OAuth subscription with a
    /// refresh token).
    Automatic,
    /// No automatic refresh; the user must re-login when it expires.
    ManualRelogin,
    /// Refresh is not a concept here (static API key).
    NotApplicable,
}

/// How ready a provider is to actually serve inference. Ordered, ascending.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthReadinessLevel {
    /// No credential.
    #[default]
    None,
    /// A credential is present but unvalidated.
    CredentialPresent,
    /// Token obtained / login complete, but a provider-specific setup step
    /// may still be pending (e.g. Gemini project onboarding).
    Authenticated,
    /// Ready to issue inference requests.
    RequestValid,
}
