//! `coco-tui-ui` — pure, domain-free presentational primitives for the coco TUI.
//!
//! The seam is "plain values in, `ratatui` out": this crate holds no `AppState`,
//! no i18n, and no application dependencies. It owns the reusable rendering
//! primitives (color adaptation, width-aware text, frame pacing, the surface
//! paint engine) that the `coco-tui` shell drives with already-projected data.

/// Pure metric predicates for the render benchmarks (probe trust chain).
/// Gated behind `testing` — never ships in release; `app/tui` enables it.
#[cfg(any(test, feature = "testing"))]
pub mod bench_metrics;
pub mod clipboard;
pub mod clipboard_copy;
pub mod clock;
pub mod color;
pub mod constants;
pub mod diff;
pub mod display;
pub mod double_press;
pub mod engine;
pub mod frame_rate_limiter;
pub mod panic_guard;
pub mod paste;
pub mod style;
pub mod system_theme;
pub mod theme;
pub mod truncate;
pub mod widgets;
