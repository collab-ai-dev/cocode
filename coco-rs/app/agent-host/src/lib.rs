//! Agent-session host for the `coco` surfaces.
//!
//! Owns session-runtime construction, AppServer request handling for local and
//! remote adapters, and protocol-neutral application behavior. The `coco-cli`
//! package owns only process startup, clap dispatch, terminal presentation, and
//! surface wiring.

// Responsibility-grouped module subdirectories.
pub mod client;
pub mod host;
pub mod integrations;
pub mod lifecycle;
pub mod session;

// Kept at crate root.
pub mod event_hub;
pub mod headless;
mod options;
pub mod paths;

#[cfg(test)]
mod test_support;

pub use options::AgentHostOptions;

// Facade re-exports: preserve crate-root paths after grouping flat modules
// into responsibility subdirectories.
pub use client::{local_client, tui_permission_bridge};
pub(crate) use host::app_session_runtime;
pub use host::{app_server_host, app_session, local_host, remote_host};
pub(crate) use integrations::{
    agent_handle_factory, agent_transcript_persistence, bash_tool_handle, command_queue_sink,
    coordinator_mode_resume, elicitation_hooks, file_changed_watcher, fork_dispatcher,
    hook_agent_runner, leader_inbox_poller, leader_permission, lsp_handle_adapter,
    mcp_handle_adapter, permission_rule_loader, sandbox_reload, shell_tool_selection,
    side_query_impl, team_task_list_router,
};
pub use integrations::{
    cron_tick, disk_task_output, mcp_cli, model_card_refresh, openai_model_refresh, plugin_dialog,
    plugin_watch, provider_login, sandbox_approval_bridge_tui, skill_watch, team_memory_sync,
};
pub use lifecycle::{live_permission_mode, resume_hint, resume_resolver, runtime_resume, shutdown};
pub(crate) use session::{
    at_mention_turn, session_close, session_data, session_mcp, session_memory, session_resume,
    session_start,
};
pub use session::{
    conversation_export, goal_command, session_agents, session_bootstrap, session_compaction,
    session_controls, session_dialogs, session_labels, session_messages, session_queue,
    session_rename, session_runtime, session_slash, task_runtime,
};

pub const BUILD_PACKAGE_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BUILD_GIT_HASH: &str = env!("COCO_BUILD_GIT_HASH");
pub const BUILD_GIT_DATE: &str = env!("COCO_BUILD_GIT_DATE");
pub const BUILD_GIT_SUBJECT: &str = env!("COCO_BUILD_GIT_SUBJECT");
pub const BUILD_TIME: &str = env!("COCO_BUILD_TIME");

pub fn build_provenance() -> coco_utils_common::BuildProvenance {
    coco_utils_common::BuildProvenance::new(
        BUILD_PACKAGE_VERSION,
        BUILD_GIT_HASH,
        BUILD_GIT_DATE,
        BUILD_GIT_SUBJECT,
        BUILD_TIME,
    )
}
