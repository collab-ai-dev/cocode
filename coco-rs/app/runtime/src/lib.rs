//! Process-, project-, and session-scoped runtime ownership.
//!
//! Process and project ownership live here today. Session construction is
//! being extracted from `coco-cli` behind the same crate boundary.

mod bootstrap;
mod process_runtime;
mod project_services;
mod workspace;

pub use bootstrap::BootstrapError;
pub use bootstrap::BootstrapSource;
pub use bootstrap::SessionRuntimeBootstrap;
pub use bootstrap::SessionRuntimeBootstrapBuild;
pub use bootstrap::StartupSnapshotSource;
pub use process_runtime::ProcessRuntime;
pub use project_services::ProjectConfigSnapshot;
pub use project_services::ProjectRegistry;
pub use project_services::ProjectRegistryManager;
pub use project_services::ProjectServices;
pub use project_services::project_registry;
pub use project_services::standard_agent_search_paths_with_plugins;
pub use workspace::SessionWorkspace;
pub use workspace::git_root_for;
pub use workspace::project_paths;
pub use workspace::resolve_project_root;
pub use workspace::runtime_paths;
pub use workspace::settings_roots_for_cwd;
