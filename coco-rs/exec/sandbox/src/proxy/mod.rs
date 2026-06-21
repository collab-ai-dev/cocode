//! Network proxy for sandboxed command network filtering.
//!
//! Provides HTTP CONNECT and SOCKS5 proxy servers that enforce
//! domain-based allow/deny filtering for sandboxed commands.
//! On Linux, includes socat-based bridges for proxy access across
//! network namespaces created by bubblewrap `--unshare-net`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub mod bridge;
mod filter;
mod server;

pub use bridge::BridgeManager;
pub use bridge::BridgePorts;
pub use filter::DomainFilter;
pub use server::ProxyServer;

/// Async callback invoked by the egress proxy when a CONNECT targets a host the
/// [`DomainFilter`] denies. Returning `true` lets the connection proceed (the
/// user approved it); `false` keeps the static 403 / SOCKS5 refusal. Built from
/// the installed [`bridge::SandboxApprovalBridge`] only when one is present, so
/// the unbridged path stays fail-closed.
///
/// Surfaces "Allow network connection to {host}?" on a denied CONNECT.
/// Host-only payload matches `SandboxNetworkAccess` tool input `{ host }`.
pub type NetworkAskCallback =
    Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync>;
