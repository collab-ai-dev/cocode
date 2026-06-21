//! MCP authentication status enum.
//!
//! Inlined from cocode-protocol to avoid cross-workspace dependency.

use std::fmt;

use serde::Deserialize;
use serde::Serialize;

/// Authentication status for an MCP server connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum McpAuthStatus {
    /// OAuth authentication is not supported by this server.
    #[default]
    Unsupported,
    /// Server supports OAuth and user has valid tokens.
    OAuth,
    /// Server uses a bearer token (e.g. via env var).
    BearerToken,
    /// Server supports OAuth but user has no tokens yet.
    NotLoggedIn,
}

impl fmt::Display for McpAuthStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => f.write_str("Unsupported"),
            Self::OAuth => f.write_str("OAuth"),
            Self::BearerToken => f.write_str("BearerToken"),
            Self::NotLoggedIn => f.write_str("NotLoggedIn"),
        }
    }
}
