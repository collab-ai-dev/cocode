//! Reminder coverage at the CLI (bare-engine) layer.
//!
//! These scenarios exercise the engine-config-driven and
//! settings-driven reminders that don't require `SessionRuntime`'s
//! cross-crate state (hooks, swarm, …) — that coverage lives in the
//! SDK and TUI suites.
//!
//! Each test runs a single short turn through `QueryEngine::run_with_events`
//! and asserts on `QueryResult.final_messages` for the expected
//! `AttachmentKind` reminder injection. Reminders land in history
//! before the turn's API call, so a single turn is enough to observe
//! them.
//!
//! Reminders deliberately not covered here:
//! - `companion_intro` — engine wires `companion_name`/`companion_species`
//!   to `None` until a future Buddy crate ships
//!   (`engine_turn_reminders.rs:425`); the reminder cannot fire today.
//! - `compaction_reminder` — needs `context_window >= 1_000_000` AND
//!   `used_tokens * 4 >= effective_context_window`. Triggering this on
//!   real LLM requires ~225K input tokens, ~100× the cost of a normal
//!   live test. `coco-system-reminder` unit tests cover the gate logic.
//! - `already_read_file` — silent reminder; routed to display-only sink
//!   (tracing-only), never reaches `final_messages`. Unit tests cover
//!   the metadata payload.
//! - `output_token_usage` — engine wires `output_token_budget` to
//!   `None` until a future TOKEN_BUDGET-equivalent ships
//!   (`engine_turn_reminders.rs:423`). Unit tests cover the gate.

pub mod auto_mode;
pub mod budget_usd;
pub mod critical_instruction;
pub mod output_style;
pub mod plan_mode;
pub mod skill_listing;
pub mod token_usage;
pub mod ultrathink_effort;
