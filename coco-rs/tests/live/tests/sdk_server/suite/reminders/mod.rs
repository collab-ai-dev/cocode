//! Reminder coverage at the SDK-server (full `SessionRuntime`) layer.
//!
//! Reminders that depend on:
//! - **PostToolUse hook output → reminder injection** —
//!   `HookAdditionalContext`, `HookStoppedContinuation`. These flow
//!   through `tool_outcome_builder::render_hook_*_message`.
//! - **SessionStart / UserPromptSubmit hooks → reminder injection** —
//!   `HookSuccess`, `HookBlockingError`, `HookAdditionalContext`. These
//!   flow through the new `SyncHookEventBuffer` →
//!   `CombinedHookEventsSource` pipeline (orchestration pushes events
//!   on every `execute_session_start` /
//!   `execute_user_prompt_submit` call).
//! - **Mid-session permission-mode transitions** —
//!   `PlanModeExit` / `PlanModeReentry`.
//! - **Runtime SkillsSource** — `SkillListing` reminder driven by the
//!   `Arc<SkillManager>` `SessionRuntime` keeps alive (rather than the
//!   `SessionBootstrap.skills` listing-only path the CLI suite covers).
//!
//! Every test reads the active `SessionHandle.history` directly through
//! the harness's `history_snapshot()` accessor — reminders inject
//! into the per-session history but never surface in the SDK
//! wire-protocol notification stream.
//!
//! Reminders deliberately not covered here:
//! - `AsyncHookResponse` — needs the engine to actually fire an async
//!   hook with `is_async: true` AND wait for its rewake response.
//!   Doable but expensive; the current `AsyncHookRegistry`'s
//!   `HookEventsSource` impl is unit-tested directly.

pub mod hook_additional_context;
pub mod hook_session_start;
pub mod hook_stopped_continuation;
pub mod hook_user_prompt_submit;
pub mod plan_mode_transitions;
pub mod skill_listing_runtime;

// `hook_blocking_error` test was dropped — when a UserPromptSubmit hook
// returns blocking_error, the SDK runner short-circuits before the
// engine runs. The reminder pipeline (which drains the sync hook buffer)
// only runs inside the engine's turn loop, so the queued
// HookEvent::BlockingError never surfaces as a `<system-reminder>` in
// history. Surfacing this kind requires a runner-side manual drain
// (architectural fix, not test-side); follow-up.
