pub mod agent;
pub mod apply_patch;
pub mod ask_user_question;
pub(crate) mod background_task;
pub mod bash;
pub mod bash_advanced;
pub mod config;
pub mod edit;
pub mod edit_utils;
pub(crate) mod file_filter;
pub mod glob;
pub mod grep;
pub mod lsp;
pub mod lsp_tool;
pub mod mcp_tools;
pub mod notebook_edit;
pub mod plan_mode;
pub mod powershell;
pub mod powershell_tool;
pub mod read;
pub mod read_permissions;
pub(crate) mod sandbox_preflight;
pub mod scheduling;
pub mod send_user_message;
pub mod shell_cwd;
pub mod shell_render;
pub mod shell_tools;
pub mod skill_advanced;
pub mod structured_output;
pub mod task_tools;
pub mod tool_search;
pub mod verify_plan_execution;
pub mod web;
pub mod worktree;
pub mod write;
pub(crate) mod write_permissions;

// File I/O (8 — `ApplyPatchTool` is gated to gpt-5 family)
pub use apply_patch::ApplyPatchTool;
pub use bash::BashTool;
pub use edit::EditTool;
pub use glob::GlobTool;
pub use grep::GrepTool;
pub use notebook_edit::NotebookEditTool;
pub use read::ReadTool;
pub use write::WriteTool;

// Web (2)
pub use web::WebFetchTool;
pub use web::WebSearchTool;

// Agent & Team (5) — schema/validation/result-formatting wrappers only.
// The catalog (definitions, prompt, filter, fork, transcript) lives in
// `coco-subagent`; the spawn lifecycle lives in `app/state/swarm` (and
// will move to the future `root/coordinator` crate in PR #3).
pub use agent::AgentTool;
pub use agent::SendMessageTool;
pub use agent::SkillTool;
pub use agent::TeamCreateTool;
pub use agent::TeamDeleteTool;

// Task Management (7)
pub use task_tools::TaskCreateTool;
pub use task_tools::TaskGetTool;
pub use task_tools::TaskListTool;
pub use task_tools::TaskOutputTool;
pub use task_tools::TaskStopTool;
pub use task_tools::TaskUpdateTool;
pub use task_tools::TodoWriteTool;

// Plan mode (2 registered + 1 conditional/unregistered helper)
pub use plan_mode::EnterPlanModeTool;
pub use plan_mode::ExitPlanModeTool;
pub use plan_mode::build_enter_plan_mode_patch;
pub use verify_plan_execution::VerifyPlanExecutionTool;

// Worktree (2)
pub use worktree::EnterWorktreeTool;
pub use worktree::ExitWorktreeTool;

// Utility (5)
pub use ask_user_question::AskUserQuestionTool;
pub use config::ConfigTool;
pub use lsp_tool::LspTool;
pub use send_user_message::SendUserMessageTool;
pub use tool_search::ToolSearchTool;

// MCP management (4)
pub use mcp_tools::ListMcpResourcesTool;
pub use mcp_tools::McpAuthServerTool;
pub use mcp_tools::McpAuthTool;
pub use mcp_tools::McpTool;
pub use mcp_tools::ReadMcpResourceTool;

// Scheduling (4)
pub use scheduling::CronCreateTool;
pub use scheduling::CronDeleteTool;
pub use scheduling::CronListTool;
pub use scheduling::RemoteTriggerTool;

// Shell (4)
pub use shell_tools::PowerShellTool;
pub use shell_tools::ReplTool;
pub use shell_tools::SleepTool;

// Synthetic (1) — `StructuredOutputTool` is conditionally registered
// (non-interactive sessions with `--json-schema`); see
// `crate::register_structured_output_tool`.
pub use structured_output::StructuredOutputTool;

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
