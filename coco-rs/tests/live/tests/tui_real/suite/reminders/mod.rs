//! Reminder coverage at the TUI (full prod path) layer.
//!
//! These reminders are sourced from the **user prompt text** —
//! `coco_context::user_input::process_user_input` parses `@…` mentions
//! out of the latest user message in history and the engine's reminder
//! pipeline emits the right `Mention*` reminder per mention type.
//!
//! Reminders deliberately not covered here:
//! - `AsyncHookResponse` — needs an async-hook + rewake round-trip
//!   spread across two turns. Unit-tested in `coco-hooks` against
//!   `AsyncHookRegistry::HookEventsSource`. The TUI harness can be
//!   extended to drive this once a sync wait-for-rewake helper is
//!   added.
//! - `McpResources` — needs `@server:uri` syntax + a connected MCP
//!   server. The TUI harness builds with `mcp_handle: None`.

pub mod agent_mention;
pub mod at_mention_file;
pub mod nested_memory;
