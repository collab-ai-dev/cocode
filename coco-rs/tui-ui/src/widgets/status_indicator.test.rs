use super::StatusIndicator;
use super::StatusIndicatorView;
use super::fmt_elapsed_compact;
use crate::style::UiStyles;
use crate::theme::Theme;
use pretty_assertions::assert_eq;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::widgets::Widget;

#[test]
fn fmt_elapsed_compact_seconds_minutes_hours() {
    assert_eq!(fmt_elapsed_compact(0), "0s");
    assert_eq!(fmt_elapsed_compact(59), "59s");
    assert_eq!(fmt_elapsed_compact(60), "1m 00s");
    assert_eq!(fmt_elapsed_compact(125), "2m 05s");
    assert_eq!(fmt_elapsed_compact(3599), "59m 59s");
    assert_eq!(fmt_elapsed_compact(3600), "1h 00m 00s");
    assert_eq!(fmt_elapsed_compact(3725), "1h 02m 05s");
}

#[test]
fn fmt_elapsed_compact_clamps_negative_to_zero() {
    assert_eq!(fmt_elapsed_compact(-7), "0s");
}

#[test]
fn spinner_frame_is_deterministic_in_time() {
    // First frame at t=0.
    assert_eq!(StatusIndicator::spinner_frame(0), "⠋");
    // Same frame within one tick interval.
    assert_eq!(StatusIndicator::spinner_frame(79), "⠋");
    // Advances at the 80ms boundary.
    assert_eq!(StatusIndicator::spinner_frame(80), "⠙");
    // Spinner is bidirectional (20 frames total: forward+reverse).
    // Wraps back to the first frame after 20 * 80 = 1_600 ms.
    assert_eq!(StatusIndicator::spinner_frame(1_600), "⠋");
}

#[test]
fn spinner_frame_never_panics_on_negative() {
    let _ = StatusIndicator::spinner_frame(-5);
    let _ = StatusIndicator::spinner_frame(i64::MIN);
}

fn render(view: StatusIndicatorView<'_>, w: u16, h: u16) -> String {
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let widget = StatusIndicator::new(view, styles);
    let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test backend");
    terminal
        .draw(|f| widget.render(f.area(), f.buffer_mut()))
        .expect("draw");
    let buf = terminal.backend().buffer();
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn renders_typical_80_col_with_tokens() {
    let view = StatusIndicatorView {
        verb: "Pondering",
        elapsed_ms: 31_000,
        input_tokens: Some(1_234),
        output_tokens: 5_678,
        effort_level: Some("high"),
        show_interrupt_hint: true,
        force_show_tokens: false,
        has_running_teammates: false,
    };
    let out = render(view, 80, 1);
    // Anchor: starts with spinner glyph + verb + effort + elapsed.
    assert!(
        out.contains("Pondering with high effort"),
        "missing verb / effort: {out:?}"
    );
    assert!(
        out.contains("(31s · ↑1.2k ↓5.7k)"),
        "missing tokens: {out:?}"
    );
    assert!(out.contains("esc to interrupt"), "missing hint: {out:?}");
}

#[test]
fn hides_tokens_before_threshold() {
    // Default threshold is SHOW_TOKENS_AFTER_MS (30s). 5s should hide.
    let view = StatusIndicatorView {
        verb: "Working",
        elapsed_ms: 5_000,
        input_tokens: None,
        output_tokens: 300,
        effort_level: None,
        show_interrupt_hint: true,
        force_show_tokens: false,
        has_running_teammates: false,
    };
    let out = render(view, 80, 1);
    assert!(!out.contains("↑"), "tokens shown too early: {out:?}");
    assert!(out.contains("(5s)"), "elapsed missing: {out:?}");
}

#[test]
fn force_show_tokens_overrides_threshold() {
    let view = StatusIndicatorView {
        verb: "Working",
        elapsed_ms: 5_000,
        input_tokens: None,
        output_tokens: 300,
        effort_level: None,
        show_interrupt_hint: true,
        force_show_tokens: true,
        has_running_teammates: false,
    };
    let out = render(view, 80, 1);
    assert!(out.contains("↑… ↓300"), "verbose-token render: {out:?}");
}

#[test]
fn running_teammates_force_tokens_before_threshold() {
    // TS `SpinnerAnimationRow.tsx:179` gate:
    // `verbose || hasRunningTeammates || elapsedMs > SHOW_TOKENS_AFTER_MS`.
    // The third disjunct is below threshold (5s < 30s); the first is
    // false. `has_running_teammates = true` alone must unlock tokens.
    let view = StatusIndicatorView {
        verb: "Working",
        elapsed_ms: 5_000,
        input_tokens: None,
        output_tokens: 300,
        effort_level: None,
        show_interrupt_hint: true,
        force_show_tokens: false,
        has_running_teammates: true,
    };
    let out = render(view, 80, 1);
    assert!(
        out.contains("↑… ↓300"),
        "teammate-running render should expose tokens: {out:?}"
    );
}

#[test]
fn narrow_terminal_drops_hint_first() {
    // 55 cols is too narrow for hint + tokens; the right-most "· esc
    // to interrupt" segment must drop first.
    let view = StatusIndicatorView {
        verb: "Pondering",
        elapsed_ms: 31_000,
        input_tokens: Some(1_234),
        output_tokens: 5_678,
        effort_level: Some("high"),
        show_interrupt_hint: true,
        force_show_tokens: false,
        has_running_teammates: false,
    };
    let out = render(view, 55, 1);
    assert!(!out.contains("esc"), "hint should be dropped: {out:?}");
    assert!(
        out.contains("↑"),
        "tokens should survive first trim: {out:?}"
    );
}

#[test]
fn tighter_terminal_drops_tokens_before_elapsed() {
    let view = StatusIndicatorView {
        verb: "Pondering",
        elapsed_ms: 31_000,
        input_tokens: Some(1_234),
        output_tokens: 5_678,
        effort_level: Some("high"),
        show_interrupt_hint: true,
        force_show_tokens: false,
        has_running_teammates: false,
    };
    let out = render(view, 28, 1);
    assert!(!out.contains("↑"), "tokens should be dropped: {out:?}");
    assert!(out.contains("(31s)"), "elapsed must remain: {out:?}");
}

#[test]
fn very_narrow_drops_effort_before_truncating_load_bearing_text() {
    let view = StatusIndicatorView {
        verb: "Pondering",
        elapsed_ms: 31_000,
        input_tokens: Some(1_234),
        output_tokens: 5_678,
        effort_level: Some("high"),
        show_interrupt_hint: true,
        force_show_tokens: false,
        has_running_teammates: false,
    };
    let out = render(view, 20, 1);
    assert!(out.contains("Pondering"), "verb dropped: {out:?}");
    assert!(!out.contains("effort"), "effort should be dropped: {out:?}");
    assert!(out.contains("(31s)"), "elapsed must remain: {out:?}");
}

#[test]
fn zero_area_renders_nothing() {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    let theme = Theme::default();
    let styles = UiStyles::new(&theme);
    let widget = StatusIndicator::new(StatusIndicatorView::for_verb("Working"), styles);
    let mut buf = Buffer::empty(Rect::new(0, 0, 0, 0));
    widget.render(Rect::new(0, 0, 0, 0), &mut buf);
    // Did not panic; nothing to assert on the empty buffer.
}
