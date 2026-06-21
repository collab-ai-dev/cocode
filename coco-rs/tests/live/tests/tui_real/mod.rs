//! Real-LLM TUI integration suite.
//!
//! Where `tui_mock.rs` runs the TUI against a `ScriptedModel` (no API
//! key, no network), this suite drives the **same TUI surface** with a
//! real provider — DeepSeek by default — through the production
//! `coco_cli` bootstrap path:
//!
//! ```text
//! Cli::parse_from(argv)
//!   → headless::build_runtime_config_for_cli       (settings + env + flags)
//!   → session_bootstrap::build_engine_resources    (RuntimeConfig, full ToolRegistry,
//!                                                    system prompt, command registry,
//!                                                    startup permission state)
//!   → session_runtime::SessionRuntime::build        (per-session subsystems:
//!                                                    HookRegistry from settings.json,
//!                                                    FileHistory, ToolAppState, …)
//!   → install_session_late_binds                    (task runtime, transcript store,
//!                                                    fork dispatcher, agent-team)
//!   → driver loop (SubmitInput / ApprovalResponse / Interrupt / Shutdown)
//!   → handle_core_event → AppState → native-surface test render
//! ```
//!
//! Skipped vs `tui_mock`:
//! - `App::run` (opens crossterm raw-mode stdin) — incompatible with a
//!   programmatic test harness. We use `AppState::new()` + native-surface
//!   rendering so the rendering pipeline is exercised but I/O isn't.
//! - Slash-command interception (`dispatch_slash_command`) is private
//!   to `tui_runner.rs`. Slash command dispatch has dedicated coverage
//!   in `coco-commands` unit tests; this suite focuses on the real-LLM
//!   round-trip.
//!
//! What this suite is for:
//! - Validating the **end-to-end real path** from prompt to model to
//!   tool execution to TUI render. Provider HTTP, tool dispatch, hook
//!   orchestration, permission bridge round-trip, AppState fold,
//!   render output — all real.
//! - Catching regressions that mock LLMs cannot detect: model-side
//!   tool-call extraction, real streaming chunking, real tool-result
//!   feedback into the next turn, real CLAUDE.md surfacing.
//!
//! Run with:
//! ```bash
//! DEEPSEEK_API_KEY=... cargo test -p coco-tests-live --test tui_real_deepseek
//! ```

pub mod harness;
pub mod suite;
