//! IDE bridge (VS Code, JetBrains) and REPL bridge for SDK/daemon callers.
//!
//! Provides message types and a server skeleton for communication
//! between IDE extensions and the coco agent, plus a REPL bridge
//! for headless/non-TUI communication with SDK consumers.

pub mod error;
pub mod jwt_utils;
pub mod permission_callbacks;
pub mod protocol;
pub mod repl;
pub mod server;
pub mod trusted_device;
pub mod work_secret;

pub use error::BridgeError;
pub use jwt_utils::Claims;
pub use jwt_utils::JwtError;

/// Crate-local Result alias. Default error is `BridgeError`; the open
/// generic preserves `Result::ok` and 2-arg `Result<T, E>` resolution.
pub type Result<T, E = BridgeError> = std::result::Result<T, E>;
pub use permission_callbacks::BridgeDecision;
pub use permission_callbacks::BridgePermissionRequest;
pub use permission_callbacks::BridgePermissionResponse;
pub use permission_callbacks::BridgeRiskLevel;
pub use protocol::BridgeInMessage;
pub use protocol::BridgeOutMessage;
pub use protocol::BridgeTransport;
pub use repl::BridgeState;
pub use repl::ControlError;
pub use repl::ControlRequest;
pub use repl::ControlRequestHandler;
pub use repl::RejectingControlHandler;
pub use repl::ReplBridge;
pub use repl::ReplInMessage;
pub use repl::ReplOutMessage;
pub use repl::dispatch_control;
pub use server::BridgeServer;
pub use trusted_device::TrustedDevice;
pub use trusted_device::TrustedDeviceStore;
pub use work_secret::account_name_for_workspace;
pub use work_secret::derive_secret_from_material;
pub use work_secret::generate_fresh_secret;
