//! Process-, project-, and session-scoped runtime ownership.
//!
//! Process and project ownership live here today. Session construction is
//! being extracted from `coco-cli` behind the same crate boundary.

mod process_runtime;
mod project_services;

pub use process_runtime::ProcessRuntime;
pub use project_services::ProjectConfigSnapshot;
pub use project_services::ProjectRegistry;
pub use project_services::ProjectRegistryManager;
pub use project_services::ProjectServices;
pub use project_services::project_registry;
pub use project_services::standard_agent_search_paths_with_plugins;
