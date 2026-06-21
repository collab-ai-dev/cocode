//! Main-turn status indicator.
//!
//! Renders a single line: `{spinner} {verb}{effort?}… ({elapsed} · {tokens?})
//! · esc to interrupt`. Visible only while a main turn is running.
//!
//! Notable behaviors:
//! - `fmt_elapsed_compact` — compact elapsed time formatting
//! - Width-aware degradation: trim the interrupt hint first, then the
//!   token block, then the effort suffix.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;

use crate::style::UiStyles;

/// Token-display threshold. Below this elapsed time the token segment
/// is hidden unless `force_show_tokens` (verbose) is set or a teammate
/// is actively running.
///
/// 30 seconds: short turns don't need the clutter.
pub const SHOW_TOKENS_AFTER_MS: i64 = 30_000;

/// Bidirectional braille spinner — 10 forward frames + 10 reverse so the
/// glyph "bounces" instead of restarting each loop.
const SPINNER_FRAMES: &[&str] = &[
    "⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", "⠏", "⠇", "⠧", "⠦", "⠴", "⠼", "⠸", "⠹", "⠙",
    "⠋",
];
const SPINNER_INTERVAL_MS: i64 = 80;

/// Borrowed view-model for [`StatusIndicator`].
#[derive(Debug, Clone, Copy)]
pub struct StatusIndicatorView<'a> {
    /// Random present-participle verb sampled once per turn. TS:
    /// `Spinner.tsx:166` (`useState` initializer).
    pub verb: &'a str,
    /// Milliseconds since the turn started. Drives both the spinner
    /// frame and the elapsed display.
    pub elapsed_ms: i64,
    /// Input tokens for this running turn when known. During the live
    /// streaming phase this is usually still pending, so renderers show
    /// an ellipsis instead of borrowing completed session totals.
    pub input_tokens: Option<i64>,
    /// Approximate output tokens for this running turn. Exact completed
    /// output usage lives in the footer and information pane.
    pub output_tokens: i64,
    /// `low` / `medium` / `high` — appended as ` with X effort`. None
    /// means no effort modifier was selected. TS `effort.ts:188-196`.
    pub effort_level: Option<&'a str>,
    /// Whether to render the `esc to interrupt` hint at the end of
    /// the line. False when the turn is in an uninterruptible state
    /// (e.g. compaction).
    pub show_interrupt_hint: bool,
    /// Force token display regardless of `SHOW_TOKENS_AFTER_MS`. TS
    /// uses this for the `verbose` flag.
    pub force_show_tokens: bool,
    /// True when at least one teammate is currently running. TS gates
    /// the token block on this in addition to verbose + elapsed
    /// (`SpinnerAnimationRow.tsx:179`:
    /// `verbose || hasRunningTeammates || effectiveElapsedMs > SHOW_TOKENS_AFTER_MS`).
    pub has_running_teammates: bool,
}

impl<'a> StatusIndicatorView<'a> {
    /// Convenience for tests: most fields default.
    #[cfg(test)]
    pub fn for_verb(verb: &'a str) -> Self {
        Self {
            verb,
            elapsed_ms: 0,
            input_tokens: None,
            output_tokens: 0,
            effort_level: None,
            show_interrupt_hint: true,
            force_show_tokens: false,
            has_running_teammates: false,
        }
    }
}

/// Render the in-progress status line above the activity panel.
pub struct StatusIndicator<'a> {
    view: StatusIndicatorView<'a>,
    styles: UiStyles<'a>,
}

impl<'a> StatusIndicator<'a> {
    pub fn new(view: StatusIndicatorView<'a>, styles: UiStyles<'a>) -> Self {
        Self { view, styles }
    }

    /// Pure-function frame selection so callers (and tests) can pick
    /// a spinner glyph without instantiating any state.
    pub fn spinner_frame(elapsed_ms: i64) -> &'static str {
        let len = SPINNER_FRAMES.len() as i64;
        let idx = ((elapsed_ms.max(0) / SPINNER_INTERVAL_MS) % len) as usize;
        SPINNER_FRAMES[idx]
    }
}

impl Widget for StatusIndicator<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let frame = StatusIndicator::spinner_frame(self.view.elapsed_ms);
        let elapsed = fmt_elapsed_compact(self.view.elapsed_ms / 1000);
        let has_tokens = self.view.input_tokens.unwrap_or(0) > 0 || self.view.output_tokens > 0;
        let want_tokens = has_tokens
            && (self.view.force_show_tokens
                || self.view.has_running_teammates
                || self.view.elapsed_ms >= SHOW_TOKENS_AFTER_MS);

        // Width-aware degradation: shed the right-most optional spans
        // until the line fits. Order is hint → tokens → effort. The
        // verb + elapsed are load-bearing — never trimmed; if the
        // terminal is narrower than that, ratatui truncates.
        let max_w = usize::from(area.width);
        let mut include_hint = self.view.show_interrupt_hint;
        let mut include_tokens = want_tokens;
        let mut include_effort = self.view.effort_level.is_some();
        let mut line = build_line(
            &self.view,
            self.styles,
            frame,
            &elapsed,
            include_hint,
            include_tokens,
            include_effort,
        );
        if line_width(&line) > max_w && include_hint {
            include_hint = false;
            line = build_line(
                &self.view,
                self.styles,
                frame,
                &elapsed,
                include_hint,
                include_tokens,
                include_effort,
            );
        }
        if line_width(&line) > max_w && include_tokens {
            include_tokens = false;
            line = build_line(
                &self.view,
                self.styles,
                frame,
                &elapsed,
                include_hint,
                include_tokens,
                include_effort,
            );
        }
        if line_width(&line) > max_w && include_effort {
            include_effort = false;
            line = build_line(
                &self.view,
                self.styles,
                frame,
                &elapsed,
                include_hint,
                include_tokens,
                include_effort,
            );
        }

        Paragraph::new(line).render(area, buf);
    }
}

/// Hours / minutes / seconds compact formatter. Verbatim port of
/// codex `status_indicator_widget.rs:fmt_elapsed_compact`.
pub fn fmt_elapsed_compact(elapsed_secs: i64) -> String {
    let elapsed_secs = elapsed_secs.max(0) as u64;
    if elapsed_secs < 60 {
        return format!("{elapsed_secs}s");
    }
    if elapsed_secs < 3600 {
        let minutes = elapsed_secs / 60;
        let seconds = elapsed_secs % 60;
        return format!("{minutes}m {seconds:02}s");
    }
    let hours = elapsed_secs / 3600;
    let minutes = (elapsed_secs % 3600) / 60;
    let seconds = elapsed_secs % 60;
    format!("{hours}h {minutes:02}m {seconds:02}s")
}

/// Compact token count: `1234 → 1.2k`, `999 → 999`, negative → `0`.
/// Local copy of `presentation::activity::format_short_tokens` to
/// avoid a cross-module pub leak.
fn fmt_compact_tokens(n: i64) -> String {
    let n = n.max(0);
    if n < 1_000 {
        return n.to_string();
    }
    format!("{:.1}k", n as f64 / 1_000.0)
}

fn line_width(line: &Line<'_>) -> usize {
    line.spans.iter().map(|s| s.content.width()).sum()
}

fn build_line<'a>(
    view: &StatusIndicatorView<'a>,
    styles: UiStyles<'a>,
    frame: &'static str,
    elapsed: &str,
    include_hint: bool,
    include_tokens: bool,
    include_effort: bool,
) -> Line<'a> {
    let mut spans: Vec<Span<'a>> = Vec::with_capacity(12);
    spans.push(Span::raw(frame).fg(styles.tool_running()));
    spans.push(Span::raw(" "));
    spans.push(Span::raw(view.verb).fg(styles.text()));
    if include_effort && let Some(effort) = view.effort_level {
        spans.push(Span::raw(format!(" with {effort} effort")).fg(styles.dim()));
    }
    spans.push(Span::raw("… "));
    let paren = if include_tokens {
        let input = view
            .input_tokens
            .map(fmt_compact_tokens)
            .unwrap_or_else(|| "…".to_string());
        format!(
            "({elapsed} · ↑{} ↓{})",
            input,
            fmt_compact_tokens(view.output_tokens)
        )
    } else {
        format!("({elapsed})")
    };
    spans.push(Span::raw(paren).fg(styles.dim()));
    if include_hint {
        spans.push(Span::raw(" · ").fg(styles.dim()));
        spans.push(Span::raw("esc").fg(styles.text()));
        spans.push(Span::raw(" to interrupt").fg(styles.dim()));
    }
    Line::from(spans)
}

#[cfg(test)]
#[path = "status_indicator.test.rs"]
mod tests;
