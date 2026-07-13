//! Agent-session host for the `coco` surfaces.
//!
//! Owns session-runtime construction, AppServer request handling for local and
//! remote adapters, and protocol-neutral application behavior. The `coco-cli`
//! package owns only process startup, clap dispatch, terminal presentation, and
//! surface wiring.

pub mod agent_handle_factory;
pub mod agent_transcript_persistence;
pub mod app_server_host;
pub mod app_session;
pub mod app_session_runtime;
pub mod at_mention_turn;
pub mod bash_tool_handle;
pub mod command_queue_sink;
pub mod conversation_export;
pub mod coordinator_mode_resume;
pub mod cron_tick;
pub mod disk_task_output;
pub mod elicitation_hooks;
pub mod event_hub;
pub mod file_changed_watcher;
pub mod fork_dispatcher;
pub mod goal_command;
pub mod headless;
pub mod hook_agent_runner;
pub mod leader_inbox_poller;
pub mod leader_permission;
pub mod live_permission_mode;
pub mod local_client;
pub mod lsp_handle_adapter;
pub mod mcp_cli;
pub mod mcp_handle_adapter;
pub mod model_card_refresh;
pub mod openai_model_refresh;
mod options;
pub mod output;
pub mod paths;
pub mod permission_rule_loader;
pub mod plugin_dialog;
pub mod plugin_watch;
pub mod provider_login;
pub mod remote_host;
pub mod resume_hint;
pub mod resume_resolver;
pub mod runtime_resume;
pub mod sandbox_approval_bridge_tui;
pub mod sandbox_reload;
pub mod session_agents;
pub mod session_archive;
pub mod session_bootstrap;
pub mod session_compaction;
pub mod session_controls;
pub mod session_data;
pub mod session_dialogs;
pub mod session_labels;
pub mod session_mcp;
pub mod session_memory;
pub mod session_messages;
pub mod session_queue;
pub mod session_rename;
pub mod session_replacement;
pub mod session_resume;
pub mod session_runtime;
pub mod session_slash;
pub mod session_start;
pub mod shell_tool_selection;
pub mod shutdown;
pub mod side_query_impl;
pub mod side_question;
pub mod skill_watch;
pub mod task_runtime;
pub mod team_memory_sync;
pub mod team_task_list_router;
pub mod teammate_inbox_pump;
pub mod tui_permission_bridge;
pub mod voice_bootstrap;

pub use options::AgentHostOptions;

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
