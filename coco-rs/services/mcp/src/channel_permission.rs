//! Channel permission relay for MCP servers.
//!
//! Allows callers to approve/deny MCP server access to specific channels
//! (e.g. Slack channels, GitHub repos) at runtime.

use std::future::Future;

use serde::Deserialize;
use serde::Serialize;

/// A permission grant/deny for a specific server+channel pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelPermission {
    pub server_name: String,
    pub channel: String,
    pub allowed: bool,
}

/// Relay for checking and requesting channel-level permissions on MCP servers.
///
/// Implementations may consult an in-memory cache, prompt the user via TUI,
/// or call an external policy service.
pub trait ChannelPermissionRelay: Send + Sync {
    /// Synchronously check whether `server` is allowed to access `channel`.
    ///
    /// Returns `true` if permission was previously granted, `false` otherwise.
    fn check_permission(&self, server: &str, channel: &str) -> bool;

    /// Asynchronously request permission for `server` to access `channel`.
    ///
    /// This may trigger a user prompt. Returns `true` if the user grants access.
    fn request_permission(&self, server: &str, channel: &str) -> impl Future<Output = bool> + Send;
}

/// Default no-op relay that denies all channel permissions.
///
/// Useful as a placeholder before the real relay is wired in.
pub struct DenyAllRelay;

impl ChannelPermissionRelay for DenyAllRelay {
    fn check_permission(&self, _server: &str, _channel: &str) -> bool {
        false
    }

    async fn request_permission(&self, _server: &str, _channel: &str) -> bool {
        false
    }
}

/// In-memory relay backed by a list of pre-approved permissions.
///
/// Checks are synchronous lookups; `request_permission` always returns `false`
/// (no interactive prompting).
pub struct StaticPermissionRelay {
    permissions: Vec<ChannelPermission>,
}

impl StaticPermissionRelay {
    pub fn new(permissions: Vec<ChannelPermission>) -> Self {
        Self { permissions }
    }
}

impl ChannelPermissionRelay for StaticPermissionRelay {
    fn check_permission(&self, server: &str, channel: &str) -> bool {
        self.permissions
            .iter()
            .any(|p| p.server_name == server && p.channel == channel && p.allowed)
    }

    async fn request_permission(&self, _server: &str, _channel: &str) -> bool {
        false
    }
}

#[cfg(test)]
#[path = "channel_permission.test.rs"]
mod tests;
