//! SDK server — NDJSON-over-stdio bidirectional control protocol.
//!
//! This crate implements the SDK connection adapter. It accepts JSON-RPC
//! frames from SDK clients (Python SDK, IDE extensions, sidecar transports,
//! etc.) over NDJSON transports, bridges them through the AppServer JSON-RPC
//! adapter, dispatches typed `ClientRequest`s to the shared AppServer host
//! handler, and writes JSON-RPC responses plus `CoreEvent` notifications back
//! through the ordered SDK writer.
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
//! Session/runtime behavior belongs to `coco-agent-host`; this crate owns only
//! SDK transport, callback correlation, outbound ordering, and bridge wiring.
//!
//! See `event-system-design.md` §5 for the control protocol catalog and
//! `coco-types/src/{jsonrpc,client_request,server_request}.rs` for the
//! wire types.

mod app_server_transport;
pub mod dispatcher;
mod event_renderer;
mod sidecar;
pub mod transport;

pub use app_server_transport::RemoteAppServerBridgeError;
pub use dispatcher::SdkServer;
pub use sidecar::{SdkSidecarConfig, SdkSidecarError, SdkSidecarListeners};
pub use transport::{InMemoryTransport, SdkTransport, StdioTransport, TransportError};
