//! Sandbox enforcement for the coco agent.
//!
//! This crate provides:
//! - Sandbox configuration (enforcement levels, writable roots, network access)
//! - Permission checking for file and network operations
//! - Platform-specific sandbox enforcement (Seatbelt on macOS, bubblewrap on Linux)
//! - Bootstrap lifecycle with 4-gate enable check
//! - Dependency detection for platform-specific binaries
//! - Violation tracking via ring buffer store
//! - Runtime state management shared across the system via `Arc`

pub mod adapter;
pub mod bootstrap;
pub mod bridge;
pub mod checker;
pub mod config;
pub mod deps;
pub mod error;
pub mod glob_expansion;
pub mod inner_stage;
pub mod monitor;
pub mod platform;
pub mod proxy;
pub mod seccomp;
pub mod state;
pub mod violation;

pub use adapter::{
    AdapterInputs, AdapterOutput, bare_repo_scrub_paths, build_runtime_config,
    detect_worktree_main_repo, resolve_filesystem_path, resolve_permission_rule_path,
    sandbox_unavailable_reason, scrub_bare_repo_files,
};
pub use bootstrap::EnableCheckResult;
pub use bootstrap::check_enable_gates;
pub use bootstrap::current_platform_supported;
pub use bridge::{
    NoOpSandboxApprovalBridge, SandboxApprovalBridge, SandboxApprovalBridgeRef,
    SandboxApprovalDecision, SandboxApprovalRequest, SandboxOperation,
};
pub use checker::PermissionChecker;
pub use config::EnforcementLevel;
pub use config::FilesystemConfig;
pub use config::IgnoreViolationsConfig;
pub use config::MitmProxyConfig;
pub use config::NetworkConfig;
pub use config::NetworkMode;
pub use config::SandboxBypass;
pub use config::SandboxConfig;
pub use config::SandboxSettings;
pub use config::WritableRoot;
pub use error::SandboxError;
pub use inner_stage::dispatch_or_continue;
pub use inner_stage::{APPLY_SECCOMP_ARG1, APPLY_WINDOWS_SANDBOX_ARG1};
pub use monitor::ViolationMonitor;
pub use monitor::{
    generate_command_tag, is_seccomp_violation, network_deny_violation, seccomp_violation,
};
pub use platform::SandboxPlatform;
pub use state::CommandSandboxSnapshot;
pub use state::ProxyPorts;
pub use state::SandboxState;
pub use violation::Violation;
pub use violation::ViolationStore;

// Re-export bridge types for Linux network namespace proxy bridging
pub use proxy::BridgeManager;
pub use proxy::BridgePorts;
