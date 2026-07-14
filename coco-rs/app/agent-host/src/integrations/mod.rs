//! Runtime integrations: tool/MCP/LSP handle adapters, watchers, refreshers,
//! coordinator/leader plumbing, and provider/sandbox/permission bridges.

pub(crate) mod agent_handle_factory;
pub(crate) mod agent_transcript_persistence;
pub(crate) mod bash_tool_handle;
pub(crate) mod command_queue_sink;
pub(crate) mod coordinator_mode_resume;
pub mod cron_tick;
pub mod disk_task_output;
pub(crate) mod elicitation_hooks;
pub(crate) mod file_changed_watcher;
pub(crate) mod fork_dispatcher;
pub(crate) mod hook_agent_runner;
pub(crate) mod leader_inbox_poller;
pub(crate) mod leader_permission;
pub(crate) mod lsp_handle_adapter;
pub mod mcp_cli;
pub(crate) mod mcp_handle_adapter;
pub mod model_card_refresh;
pub mod openai_model_refresh;
pub(crate) mod permission_rule_loader;
pub mod plugin_dialog;
pub mod plugin_watch;
pub mod provider_login;
pub mod sandbox_approval_bridge_tui;
pub(crate) mod sandbox_reload;
pub(crate) mod shell_tool_selection;
pub(crate) mod side_query_impl;
pub mod skill_watch;
pub mod team_memory_sync;
pub(crate) mod team_task_list_router;
