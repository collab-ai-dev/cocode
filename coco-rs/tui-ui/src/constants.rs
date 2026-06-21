//! Named constants for the TUI.
//!
//! Centralizes magic numbers used across TUI modules. Values are `i32`
//! and should be cast to `u16` at the call site when passed to ratatui.

use std::time::Duration;

// ========== Layout ==========

/// Maximum height (rows) for the multi-line input box, including borders.
pub const MAX_INPUT_HEIGHT: i32 = 10;

/// Maximum width (cols) for the inline interaction prompt box, including
/// borders. The box is left-anchored inside the available area so wide
/// terminals don't stretch its border all the way across.
pub const MAX_INTERACTION_PROMPT_WIDTH: u16 = 100;

// ========== Display Limits ==========

/// Maximum active toast notifications at once.
pub const MAX_TOASTS: i32 = 5;

/// Maximum command history entries retained.
pub const MAX_HISTORY_ENTRIES: i32 = 100;

// ========== Scrolling ==========

/// Lines to scroll per arrow-key scroll step.
pub const SCROLL_LINE_STEP: i32 = 3;

/// Lines to scroll per page-scroll step.
pub const SCROLL_PAGE_STEP: i32 = 20;

// ========== Timing ==========

/// Interval for status-bar updates, toast expiry, idle detection.
pub const TICK_INTERVAL: Duration = Duration::from_millis(250);

/// Interval for spinner animation frames while a stream is in flight.
/// Streaming reveal pacing is one line per PAINTED frame
/// (`StreamingState::advance_display`), so this cadence also bounds
/// streamed-text throughput — do not slow it while streaming.
pub const SPINNER_TICK_INTERVAL: Duration = Duration::from_millis(50);

/// Frame cadence for spinner-only phases (tool running / compacting, no
/// stream in flight): nothing on screen changes faster than the spinner
/// glyph, whose period this matches
/// (`widgets::status_indicator::SPINNER_INTERVAL_MS`). Sampling at the glyph
/// period keeps every transition visible while cutting the idle paint load
/// vs the 50ms streaming tick.
pub const SPINNER_ONLY_TICK_INTERVAL: Duration = Duration::from_millis(80);

/// Duration before an state transition gate drops.
pub const OVERLAY_TRANSITION_DURATION: Duration = Duration::from_millis(150);

// ========== Toast Durations ==========

/// Duration for informational toasts.
pub const TOAST_INFO_DURATION: Duration = Duration::from_secs(3);

/// Duration for success toasts.
pub const TOAST_SUCCESS_DURATION: Duration = Duration::from_secs(3);

/// Duration for warning toasts.
pub const TOAST_WARNING_DURATION: Duration = Duration::from_secs(5);

/// Duration for error toasts.
pub const TOAST_ERROR_DURATION: Duration = Duration::from_secs(8);

// ========== Text ==========

/// Maximum characters for inline tool description preview.
pub const TOOL_DESCRIPTION_MAX_CHARS: i32 = 40;

/// Multiplier to estimate token count from word count.
pub const THINKING_TOKEN_MULTIPLIER: f64 = 1.3;

/// Maximum body lines shown for an expanded thinking block before the
/// `…` overflow marker. TS keeps the full block in a markdown render
/// (no clamp); coco-rs clamps so a chatty model doesn't push other
/// chat rows off-screen. Bump this up if users want more context.
pub const THINKING_PREVIEW_LINES: usize = 5;

// ========== Virtual Scroll ==========

/// Number of messages to render beyond the visible viewport (buffer).
pub const VIRTUAL_SCROLL_OVERSCAN: i32 = 5;

// ========== Table ==========

/// Maximum column width for markdown tables.
pub const TABLE_MAX_COL_WIDTH: i32 = 40;

/// Minimum column width for markdown tables.
pub const TABLE_MIN_COL_WIDTH: i32 = 5;

// ========== Search ==========

/// Maximum search results shown in global search state.
pub const MAX_SEARCH_RESULTS: i32 = 50;

// ========== Double-press ==========

/// Window between the first and second press of any double-press
/// shortcut (Ctrl+C / Ctrl+D exit, double-Esc rewind, …). Consumed by
/// [`crate::double_press::DoublePressTracker`].
///
/// 800 ms matches the upstream `DOUBLE_PRESS_TIMEOUT_MS` constant.
pub const DOUBLE_PRESS_TIMEOUT: Duration = Duration::from_millis(800);

// ========== Rewind ==========

/// Maximum visible messages in the rewind message selector.
pub const REWIND_MAX_VISIBLE: i32 = 7;

/// Display-column budget for the per-row prompt label in the rewind
/// picker. The modal sits inside the chat viewport with single-line
/// border + side padding; this leaves room for the marker, suffix
/// `(N min ago)`, and the visible truncation ellipsis. TS uses
/// `truncate(messageText, columns - paddingRight=10, true)`
/// (`MessageSelector.tsx:374, 665`); since coco-rs renders to a
/// fixed-width body string (no terminal columns available here),
/// we pin a conservative budget that fits typical 100-col modal
/// widths without wrapping under ratatui's word-wrap.
pub const REWIND_DISPLAY_WIDTH_BUDGET: usize = 72;

// ========== Mouse ==========

/// Lines per mouse scroll wheel tick.
pub const MOUSE_SCROLL_LINES: i32 = 3;
