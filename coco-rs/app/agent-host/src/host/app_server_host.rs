//! Protocol-neutral AppServer host facade shared by local and remote adapters.
//!
//! Remote JSON-RPC owns connection transport and callback correlation. Local
//! surfaces should enter through this module instead of transport-specific code.

mod bootstrap_state;
mod bridge_control;
mod cli_initialize_bootstrap;
pub(crate) mod client_mcp_bridge;
pub(crate) mod config;
pub(crate) mod connection_profile;
pub(crate) mod connection_runtime_binding;
pub(crate) mod handler;
pub(crate) mod handler_error_mapping;
pub(crate) mod hook_callback_bridge;
pub(crate) mod idle_session_supervisor;
pub(crate) mod initialize_agents;
mod initialize_bootstrap;
pub(crate) mod local_bridge;
pub(crate) mod outbound;
mod permission_bridge;
pub(crate) mod remote_bridge;
pub(crate) mod request_context;
pub(crate) mod request_dispatch;
pub(crate) mod request_handlers;
pub(crate) mod request_targeting;
mod runtime_replacement;
pub(crate) mod runtime_replacement_gate;
mod sandbox_approval_bridge;
pub(crate) mod session_close;
pub(crate) mod session_data;
pub(crate) mod session_errors;
pub(crate) mod session_loading;
pub(crate) mod session_local_operations;
pub(crate) mod session_operation_error;
pub(crate) mod session_operation_input;
pub(crate) mod session_registry;
pub(crate) mod session_replace_operation;
pub(crate) mod session_request_mapping;
pub(crate) mod session_resume_operation;
pub(crate) mod session_start_operation;
mod session_store;
pub(crate) mod session_surfaces;
pub(crate) mod session_turn_executor;
mod state;
mod turn_runner;

pub use bridge_control::AppServerBridgeControlHandler;
pub use cli_initialize_bootstrap::CliInitializeBootstrap;
pub(crate) use cli_initialize_bootstrap::build_remote_initialize_bootstrap;
pub use config::APP_SERVER_TURN_DRAIN_TIMEOUT;
pub use connection_runtime_binding::install_app_server_session_runtime_state;
pub use handler::AppServerHostHandler;
pub use idle_session_supervisor::spawn_idle_session_sweep;
pub use initialize_bootstrap::InitializeBootstrap;
pub use local_bridge::AppServerLocalBridge;
pub use outbound::event_agent_id;
pub use outbound::route_app_server_session_event;
pub use outbound::{OutboundMessage, ProcessEvent};
pub use outbound::{install_session_seq_durability, spawn_app_server_local_outbound_forwarder};
pub(crate) use permission_bridge::AppServerPermissionBridge;
pub(crate) use remote_bridge::build_remote_app_server_runtime_binding;
pub(crate) use remote_bridge::open_remote_sidecar_binding;
pub(crate) use remote_bridge::shutdown_remote_app_server_host;
pub use remote_bridge::{
    RemoteAppServer, RemoteAppServerBridgeHost, RemoteAppServerConnectionBinding,
    RemoteAppServerHandle, RemoteJsonRpcAdapter, RemoteJsonRpcConnection, RemoteOutboundMessage,
    RemoteSidecarHostBinding,
};
pub use request_context::{HandlerContext, HandlerResult, SessionRequestContext};
pub use request_dispatch::dispatch_client_request;
pub use runtime_replacement::RuntimeReplacementContext;
pub use session_close::shutdown_local_app_server_sessions;
pub use session_turn_executor::SessionTurnExecutor;
pub use state::{AppServerHostState, HostInputs};
pub use turn_runner::TurnRunner;
