// Test-helper API kept stable across suites — individual scenarios use
// only a slice of the surface, but the whole crate ships them so future
// suites land without hunting for plumbing. Cargo's per-target dead-code
// pass would otherwise flag every unused getter on a per-suite basis.
#![allow(dead_code)]

//! In-process TUI integration tests.
//!
//! These suites boot the TUI's full state machine + render pipeline in a
//! real `tokio` runtime — but without the binary, without crossterm
//! raw-mode, and without a live LLM provider. The chain exercised is:
//!
//! ```text
//!  TuiHarness  →  command_tx  →  agent driver task  →  QueryEngine
//!                                                          ↓
//!                                          ToolRegistry · HookRegistry · ScriptedModel
//!                                                          ↓
//!  TuiHarness  ←  event_rx  ←  CoreEvent  ←  engine.run_with_events
//!       ↓
//!  AppState  ←  handle_core_event  (folds events into TUI model)
//!       ↓
//!  native surface renderer  ←  AppState  (paints model into test buffer)
//! ```
//!
//! The harness owns the channels the TUI and engine use to talk plus a
//! JoinHandle for the background driver. Tests call `submit("…")` to inject a
//! user prompt, `pump_until_idle()` to drain the event stream into AppState,
//! and `render_to_string()` to snapshot what the user would see on a real
//! terminal.
//!
//! `ScriptedModel` returns a deterministic queue of canned LLM responses —
//! no API key, no network. Hooks fire against real `coco-hooks` machinery
//! (Command-style hooks that touch a tempfile so tests can verify
//! Pre/PostToolUse ran with the right tool/inputs).

pub mod harness;
pub mod scripted_model;
pub mod suite;
