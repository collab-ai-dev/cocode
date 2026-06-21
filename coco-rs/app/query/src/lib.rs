//! Query engine — the multi-turn agent loop.
//!
//! The core cycle:
//! 1. Build system prompt (context)
//! 2. Normalize messages for API
//! 3. Call LLM via inference
//! 4. Parse response, extract tool calls
//! 5. Execute tools (via ToolExecutor batch partitioning)
//! 6. Check stop conditions (no tool calls, max turns, budget)
//! 7. Auto-compact if needed
//! 8. Drain command queue + inject attachments
//! 9. Loop back to 1 if tool calls, else done

pub mod agent_adapter;
pub mod budget;
pub mod command_queue;
pub mod config;
pub mod context_analysis;
pub mod emit;
pub mod engine;
mod engine_attachments;
mod engine_builder;
mod engine_compaction;
mod engine_finalize_turn;
mod engine_helpers;
pub mod engine_live_rules;
mod engine_loop_state;
mod engine_prompt;
mod engine_recovery;
mod engine_result;
mod engine_session;
mod engine_stop_hooks;
mod engine_stream_consume;
mod engine_terminal;
mod engine_tool_commit;
mod engine_tool_execution;
mod engine_turn_reminders;
mod engine_turn_request;
pub mod error_code;
pub mod fork_context;
pub mod forked_agent;
mod helpers;
pub mod history_sync;
mod hook_adapter;
mod hook_controller;
pub mod hook_llm;
mod model_runtime;
pub mod observers;
mod permission_controller;
pub mod plan_mode_reminder;
pub mod prompt_suggestion;
pub mod reminder_adapters;
pub mod sdk_types;
pub mod session_start_hooks;
mod session_state;
pub mod single_turn;
pub mod skill_runtime;
pub mod stream_accumulator;
pub mod structured_output_enforcement;
pub mod test_support;
mod tool_call_preparer;
mod tool_call_runner;
mod tool_context;
mod tool_input_normalizer;
mod tool_input_parse;
mod tool_input_validate;
mod tool_message;
mod tool_outcome_builder;
mod tool_runner;
pub mod tool_use_summary;
mod wire_tap_adapter;

pub use budget::BudgetDecision;
pub use budget::BudgetTracker;
pub use command_queue::CommandQueue;
pub use command_queue::QueuePriority;
pub use command_queue::QueuedCommand;
pub use command_queue::QueuedImage;
pub use config::ContinueReason;
pub use config::PermissionDerivation;
pub use config::QueryEngineConfig;
pub use config::QueryResult;
pub use config::SessionBootstrap;
pub use engine::QueryEngine;
pub use engine_compaction::ManualCompactRequest;
pub use engine_live_rules::EngineLiveRulesHandle;
pub use helpers::queued_command_to_attachment;
pub use helpers::queued_command_to_message;
pub use helpers::wrap_steering_messages_for_api;
pub use session_start_hooks::SessionStartHookSideEffectSink;
pub use session_start_hooks::SessionStartHookSideEffectSinkRef;
pub use session_start_hooks::SessionStartHookSideEffects;
// Re-export so the bootstrap (`app/cli`) keeps building it as
// `coco_query::WireDumpConfig` while the impl lives in `coco-wire-dump`.
pub use coco_wire_dump::WireDumpConfig;

// Re-export CoreEvent from coco-types for consumers of run_with_events().
// The old QueryEvent enum has been deleted per event-system-design.md Phase 0:
// QueryEngine now emits CoreEvent directly (3-layer Protocol/Stream/Tui dispatch).
pub use coco_types::AgentStreamEvent;
pub use coco_types::CoreEvent;
pub use coco_types::ServerNotification;
pub use stream_accumulator::StreamAccumulator;
