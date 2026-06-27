//! SDK server — NDJSON-over-stdio bidirectional control protocol.
//!
//! This module implements the server side of the Phase 2 SDK protocol. It
//! accepts `JsonRpcMessage` requests from SDK clients (Python SDK, IDE
//! extensions, etc.) over stdin, dispatches them to coco-rs handlers, and
//! writes `JsonRpcMessage` responses + CoreEvent notifications to stdout.
//!
//! Architecture:
//! ```text
//! SDK client (Python / IDE / test harness)
//!     │
//!     │ JSON-RPC over NDJSON (stdin/stdout)
//!     ▼
//! ┌───────────────────────────────┐
//! │ StdioTransport (NDJSON I/O)   │
//! │   read: JsonRpcMessage stream  │
//! │   write: JsonRpcMessage sink   │
//! └──────────┬────────────────────┘
//!            │
//!            ▼
//! ┌───────────────────────────────┐
//! │ SdkServer dispatch loop        │
//! │   ClientRequest → handler      │
//! │   CoreEvent → notification     │
//! └───────────────────────────────┘
//! ```
//!
//!
//! See `event-system-design.md` §5 for the control protocol catalog and
//! `coco-types/src/{jsonrpc,client_request,server_request}.rs` for the
//! wire types.

pub mod approval_bridge;
pub mod bridge_control;
pub mod cli_bootstrap;
pub mod dispatcher;
pub mod handlers;
pub mod outbound;
pub mod pending_map;
pub mod sandbox_approval_bridge;
pub mod sdk_hooks;
pub mod sdk_mcp;
pub mod sdk_runner;
pub mod transport;

pub use approval_bridge::SdkPermissionBridge;
pub use bridge_control::SdkBridgeControlHandler;
pub use cli_bootstrap::CliInitializeBootstrap;
pub use dispatcher::SdkServer;
pub use dispatcher::server_notification_to_jsonrpc;
pub use handlers::HandlerContext;
pub use handlers::HandlerResult;
pub use handlers::InitializeBootstrap;
pub use handlers::SdkServerState;
pub use handlers::SessionHandle;
pub use handlers::SessionStats;
pub use handlers::TurnRunner;
pub use handlers::dispatch_client_request;
pub use sandbox_approval_bridge::SdkSandboxApprovalBridge;
pub use sdk_runner::QueryEngineRunner;
pub use transport::InMemoryTransport;
pub use transport::SdkTransport;
pub use transport::StdioTransport;
pub use transport::TransportError;
