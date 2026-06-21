//! TUI integration test suites. Each module is one focused scenario
//! that drives the [`TuiHarness`] through a specific code path.
//!
//! Top-level `tui_mock.rs` wires each suite to a `#[tokio::test]` entry.

pub mod agent_team_spawn;
pub mod auto_compact_trigger;
pub mod bash_capture;
pub mod boot_render;
pub mod compact_round_trip;
pub mod empty_submit;
pub mod hook_verify;
pub mod input_editing;
pub mod interrupt_inflight;
pub mod keyboard_dispatch;
pub mod multi_turn;
pub mod one_shot;
pub mod parallel_tools;
pub mod permission_round_trip;
pub mod rewind_overlay;
pub mod slash_clear;
pub mod slash_copy;
pub mod thinking_block;
pub mod tool_chain;
pub mod tool_error;
