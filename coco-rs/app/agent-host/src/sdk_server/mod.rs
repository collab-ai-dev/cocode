//! SDK server — NDJSON-over-stdio bidirectional control protocol.
//!
//! This module implements the server side of the Phase 2 SDK protocol. It
//! accepts `JsonRpcMessage` requests from SDK clients (Python SDK, IDE
//! extensions, etc.) over stdin, bridges them through the AppServer JSON-RPC
//! adapter, dispatches typed `ClientRequest`s to coco-rs handlers, and writes
//! `JsonRpcMessage` responses + CoreEvent notifications to stdout.
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
//! │ AppServer JSON-RPC bridge      │
//! │   ClientRequest → handler      │
//! │   CoreEvent → notification     │
//! └───────────────────────────────┘
//! ```
//!
//!
//! See `event-system-design.md` §5 for the control protocol catalog and
//! `coco-types/src/{jsonrpc,client_request,server_request}.rs` for the
//! wire types.

pub(crate) mod app_server_bridge;
pub mod approval_bridge;
pub mod bridge_control;
pub mod cli_bootstrap;
pub mod dispatcher;
pub mod handlers;
mod idle_session_supervisor;
pub mod outbound;
pub mod sandbox_approval_bridge;
pub mod sdk_hooks;
pub mod sdk_mcp;
pub mod sdk_runner;
mod session_data;
mod session_lifecycle;
mod session_store;
pub mod transport;

pub use crate::session_runtime::SessionStats;
pub use app_server_bridge::{
    AppServerLocalBridge, AppServerSdkHandler, LocalAppSessionHandle, SdkAppServerBridgeError,
    install_session_seq_durability, spawn_app_server_local_outbound_forwarder,
};
pub use approval_bridge::SdkPermissionBridge;
pub use bridge_control::SdkBridgeControlHandler;
pub use cli_bootstrap::CliInitializeBootstrap;
pub use dispatcher::{SdkServer, server_notification_to_jsonrpc};
pub use handlers::{
    HandlerContext, HandlerResult, InitializeBootstrap, RuntimeReplacementContext, SdkServerState,
    TurnRunner, dispatch_client_request,
};
pub use idle_session_supervisor::spawn_idle_session_sweep;
pub use sandbox_approval_bridge::SdkSandboxApprovalBridge;
pub use sdk_runner::SessionTurnExecutor;
pub use session_lifecycle::{
    install_sdk_session_runtime_state, load_local_app_server_session_runtime,
    shutdown_local_app_server_sessions,
};
pub use transport::{InMemoryTransport, SdkTransport, StdioTransport, TransportError};
