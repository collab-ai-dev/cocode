use crossterm::cursor::SetCursorStyle;
use pretty_assertions::assert_eq;
use ratatui::backend::Backend;
use ratatui::backend::CrosstermBackend;
use ratatui::backend::TestBackend;
use ratatui::layout::Position;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use std::cell::RefCell;
use std::io;
use std::io::Write;
use std::rc::Rc;

use crate::engine::history_insert::HistoryRows;
use crate::engine::history_insert::render_history_rows;
use crate::engine::seat::SeatInputs;
use crate::engine::seat::ViewportPin;

use super::*;

fn history_rows(lines: impl IntoIterator<Item = Line<'static>>) -> HistoryRows {
    render_history_rows(lines.into_iter().collect(), 8)
}

fn history_rows_width(lines: impl IntoIterator<Item = Line<'static>>, width: u16) -> HistoryRows {
    render_history_rows(lines.into_iter().collect(), width)
}

/// Assert no non-blank visible row appears more than once in the screen
/// buffer — the duplication signature of the old tail-cache reveal.
fn assert_no_duplicate_rows(terminal: &SurfaceTerminal<TestBackend>, context: &str) {
    let buffer = terminal.backend().buffer();
    let dupes = duplicate_nonblank_rows(buffer);
    assert!(
        dupes.is_empty(),
        "duplicated visible history rows {context}: {dupes:?}\nbuffer:\n{}",
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

/// Non-blank visible rows that appear more than once in the screen buffer —
/// the duplication signature.
fn duplicate_nonblank_rows(buffer: &ratatui::buffer::Buffer) -> Vec<String> {
    let width = buffer.area.width as usize;
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut dupes = Vec::new();
    for chunk in buffer.content.chunks(width.max(1)) {
        let text = chunk
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>()
            .trim_end()
            .to_string();
        if text.is_empty() {
            continue;
        }
        let count = seen.entry(text.clone()).or_insert(0);
        *count += 1;
        if *count == 2 {
            dupes.push(text);
        }
    }
    dupes
}

#[test]
fn prompt_shrink_defers_then_append_backed_commit_does_not_duplicate() {
    // Regression pin for the permission-prompt duplication (tui-v2). The old
    // shrink path jumped the viewport down to the screen bottom and
    // back-filled the freed rows from the history tail cache — but the
    // cache's most-recent rows were ALREADY visible just above the gap, so
    // the fill painted them a second time (`h2 h3 h2 h3` on screen). Now the
    // confirm frame (no append yet) DEFERS the shrink — the viewport keeps
    // its seat, so the bottom-aligned composer never lifts off the screen
    // bottom (the input-box bounce class) — and the next frames commit
    // exactly the rows the history appends back, never repainting history.
    let screen = Size::new(8, 10);
    let backend = TestBackend::new(8, 10);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.sync_screen_size(screen);
    terminal.set_viewport_area(Rect::new(0, 8, 8, 2));
    terminal
        .insert_history_rows(&history_rows([
            Line::from("h0"),
            Line::from("h1"),
            Line::from("h2"),
            Line::from("h3"),
        ]))
        .expect("insert history");
    // Permission prompt: the grow scrolls history up (codex grows the same
    // way — `tui.rs::draw` scrolls the region above by the overflow).
    terminal
        .apply_viewport_area(Rect::new(0, 2, 8, 8), true)
        .expect("grow for prompt");

    // Confirm frame, nothing to append yet: the shrink defers wholesale.
    let confirm = terminal.seat_viewport(SeatInputs {
        screen,
        desired_height: 2,
        min_height: 2,
        max_height: 8,
        guaranteed_append_rows: 0,
    });
    assert_eq!(confirm.pin, ViewportPin::BottomPinned);
    assert_eq!(confirm.viewport, Rect::new(0, 2, 8, 8));
    assert_eq!(confirm.deferred_shrink_rows, 6);
    terminal
        .apply_viewport_area(confirm.viewport, true)
        .expect("confirm frame seat");
    assert!(terminal.seats_flush());
    assert_no_duplicate_rows(&terminal, "after the deferred prompt shrink");

    // Tool result arrives: the seat commits exactly the appended rows and
    // the same-frame insert fills them — no history is ever repainted.
    let result_frame = terminal.seat_viewport(SeatInputs {
        screen,
        desired_height: 2,
        min_height: 2,
        max_height: 8,
        guaranteed_append_rows: 2,
    });
    assert_eq!(result_frame.pin, ViewportPin::BottomPinned);
    assert_eq!(result_frame.viewport, Rect::new(0, 4, 8, 6));
    assert_eq!(result_frame.deferred_shrink_rows, 4);
    terminal
        .apply_viewport_area(result_frame.viewport, true)
        .expect("result frame seat");
    terminal
        .insert_history_rows(&history_rows([Line::from("h4"), Line::from("h5")]))
        .expect("insert tool result");
    assert_eq!(terminal.viewport_area(), Rect::new(0, 4, 8, 6));
    assert!(terminal.seats_flush());
    assert_no_duplicate_rows(&terminal, "after the append-backed commit");
    terminal.backend().assert_buffer_lines([
        "h2      ", "h3      ", "h4      ", "h5      ", "        ", "        ", "        ",
        "        ", "        ", "        ",
    ]);
}

#[test]
fn surface_terminal_draws_inside_configured_viewport() {
    let backend = TestBackend::new(12, 5);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    assert_eq!(terminal.last_known_screen_size(), Size::new(12, 5));
    terminal.set_viewport_area(Rect::new(0, 3, 12, 2));
    assert_eq!(terminal.viewport_area(), Rect::new(0, 3, 12, 2));

    terminal
        .draw_viewport(|frame| {
            frame.render_widget(Paragraph::new("hello"), frame.area());
        })
        .expect("draw");

    terminal.backend().assert_buffer_lines([
        "            ",
        "            ",
        "            ",
        "hello       ",
        "            ",
    ]);
}

#[test]
fn surface_terminal_skips_hidden_cells_after_wide_chars() {
    let backend = TestBackend::new(20, 2);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 0, 20, 2));
    terminal
        .current_buffer_mut()
        .set_string(0, 0, "❯ 你是什么模型", Style::default());

    let updates = terminal.buffer_updates();

    assert!(updates.iter().all(|(_, _, cell)| !cell.skip));
    let symbols = updates
        .iter()
        .map(|(_, _, cell)| cell.symbol())
        .collect::<String>();
    assert!(symbols.contains("你是"), "got {symbols:?}");
    assert!(!symbols.contains("你 "), "got {symbols:?}");
}

#[test]
fn surface_terminal_applies_cursor_claim() {
    let backend = TestBackend::new(8, 4);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 2, 8, 2));

    terminal
        .draw_viewport(|frame| {
            frame.set_cursor_claim(CursorClaim {
                position: Position { x: 3, y: 3 },
                style: SetCursorStyle::SteadyBar,
            });
        })
        .expect("draw");

    terminal
        .backend_mut()
        .assert_cursor_position(Position { x: 3, y: 3 });
}

#[test]
fn surface_terminal_hides_cursor_without_claim_and_homes_position() {
    let backend = TestBackend::new(8, 4);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");

    terminal.draw_viewport(|_frame| {}).expect("draw");

    terminal
        .backend_mut()
        .assert_cursor_position(Position { x: 0, y: 0 });
}

#[test]
fn visible_history_rows_are_clamped_to_rows_above_viewport() {
    let backend = TestBackend::new(10, 6);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 4, 10, 2));

    terminal.note_history_rows_inserted(3);
    assert_eq!(terminal.visible_history_rows(), 3);

    terminal.note_history_rows_inserted(10);
    assert_eq!(terminal.visible_history_rows(), 4);
}

#[test]
fn set_viewport_area_reclamps_visible_history_rows() {
    let backend = TestBackend::new(10, 8);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 5, 10, 3));
    terminal.note_history_rows_inserted(5);

    terminal.set_viewport_area(Rect::new(0, 2, 10, 3));

    assert_eq!(terminal.visible_history_rows(), 2);
}

#[test]
fn clear_owned_scrollback_resets_history_accounting_and_repaints() {
    let backend = TestBackend::new(8, 3);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 1, 8, 2));
    terminal.note_history_rows_inserted(1);
    terminal
        .draw_viewport(|frame| {
            frame.render_widget(
                Paragraph::new("stale").style(Style::default()),
                frame.area(),
            );
        })
        .expect("initial draw");

    terminal.clear_owned_scrollback().expect("clear");
    terminal
        .draw_viewport(|frame| {
            frame.render_widget(Paragraph::new("fresh"), frame.area());
        })
        .expect("redraw");

    assert_eq!(terminal.visible_history_rows(), 0);
    assert_eq!(terminal.history_bottom_y(), 0);
    assert_eq!(terminal.viewport_area(), Rect::new(0, 0, 8, 2));
    terminal
        .backend()
        .assert_buffer_lines(["fresh   ", "        ", "        "]);
}

#[test]
fn clear_viewport_to_end_preserves_history_above_viewport() {
    let backend = TestBackend::with_lines(["history ", "stale 1 ", "stale 2 "]);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 1, 8, 2));

    terminal.clear_viewport_to_end().expect("clear viewport");

    terminal
        .backend()
        .assert_buffer_lines(["history ", "        ", "        "]);
}

#[test]
fn prepare_shell_prompt_after_exit_clears_viewport_and_places_cursor_after_history() {
    let backend = TestBackend::with_lines(["history ", "stale 1 ", "stale 2 "]);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 1, 8, 2));

    terminal
        .prepare_shell_prompt_after_exit()
        .expect("prepare prompt");

    terminal
        .backend()
        .assert_buffer_lines(["history ", "        ", "        "]);
    terminal
        .backend_mut()
        .assert_cursor_position(Position { x: 0, y: 1 });
}

#[test]
fn apply_viewport_area_shrink_clears_old_live_tail() {
    let backend = TestBackend::with_lines(["history", "live 1 ", "live 2 ", "input ", "status"]);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 1, 7, 4));

    terminal
        .apply_viewport_area(Rect::new(0, 3, 7, 2), true)
        .expect("apply viewport");

    terminal
        .backend()
        .assert_buffer_lines(["history", "       ", "       ", "       ", "       "]);
}

#[test]
fn apply_viewport_area_growth_scrolls_history_before_clearing_viewport() {
    let backend = TestBackend::with_lines(["hist 1", "hist 2", "hist 3", "live 1", "input "]);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 3, 6, 2));

    terminal
        .apply_viewport_area(Rect::new(0, 1, 6, 4), true)
        .expect("apply viewport");

    terminal
        .backend()
        .assert_buffer_lines(["hist 3", "      ", "      ", "      ", "      "]);
}

#[test]
fn apply_viewport_area_closes_gap_without_scrolling_history() {
    let backend =
        TestBackend::with_lines(["hist 1", "hist 2", "      ", "      ", "live  ", "input "]);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 4, 6, 2));
    terminal.note_history_rows_inserted(2);

    terminal
        .apply_viewport_area(Rect::new(0, 2, 6, 2), true)
        .expect("apply viewport");

    assert_eq!(terminal.history_bottom_y(), 2);
    terminal
        .backend()
        .assert_buffer_lines(["hist 1", "hist 2", "      ", "      ", "      ", "      "]);
}

#[test]
fn insert_history_rows_after_viewport_shrink_closes_live_tail_gap() {
    let backend = TestBackend::new(8, 10);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 8, 8, 2));
    terminal
        .insert_history_rows(&history_rows([
            Line::from("header"),
            Line::default(),
            Line::from("❯ hello"),
            Line::default(),
        ]))
        .expect("insert first history");
    terminal
        .apply_viewport_area(Rect::new(0, 5, 8, 5), true)
        .expect("grow viewport");
    terminal
        .apply_viewport_area(Rect::new(0, 8, 8, 2), true)
        .expect("shrink viewport");

    terminal
        .insert_history_rows(&history_rows([Line::from("⏺ hi"), Line::default()]))
        .expect("insert assistant history");

    terminal.backend().assert_buffer_lines([
        "        ",
        "header  ",
        "        ",
        "❯ hello",
        "        ",
        "⏺ hi    ",
        "        ",
        "        ",
        "        ",
        "        ",
    ]);
}

#[test]
fn insert_history_rows_writes_above_viewport_and_preserves_viewport() {
    let backend = TestBackend::with_lines(["old0  ", "old1  ", "old2  ", "view0 ", "view1 "]);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 3, 6, 2));

    let inserted = terminal
        .insert_history_rows(&history_rows_width(
            [Line::from("hist0"), Line::from("hist1")],
            6,
        ))
        .expect("insert history");

    assert_eq!(inserted, 2);
    assert_eq!(terminal.visible_history_rows(), 2);
    terminal
        .backend()
        .assert_buffer_lines(["old2  ", "hist0 ", "hist1 ", "view0 ", "view1 "]);
    terminal
        .backend()
        .assert_scrollback_lines(["old0  ", "old1  "]);
}

#[test]
fn surface_terminal_reports_viewport_draw_stats() {
    let backend = TestBackend::new(8, 4);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_perf_stats_enabled(true);
    terminal.set_viewport_area(Rect::new(0, 2, 8, 2));

    terminal
        .draw_viewport(|frame| {
            frame.render_widget(Paragraph::new("hi"), frame.area());
        })
        .expect("draw");

    let stats = terminal.last_viewport_draw_stats();
    assert_eq!(stats.buffer_updates, 16);
    assert!(stats.invalidated);
    assert!(stats.diff_elapsed.as_nanos() > 0);
}

#[test]
fn surface_terminal_reports_history_insert_stats() {
    let backend = TestBackend::new(8, 6);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_perf_stats_enabled(true);
    terminal.set_viewport_area(Rect::new(0, 4, 8, 2));

    let inserted = terminal
        .insert_history_rows(&history_rows([Line::from("history line")]))
        .expect("insert history");

    let stats = terminal.last_history_insert_stats();
    assert_eq!(inserted, 2);
    assert_eq!(stats.wrapped_rows, 2);
    assert_eq!(stats.buffer_updates, 16);
    assert!(stats.invalidated);
    assert_eq!(stats.build_elapsed.as_nanos(), 0);
}

#[test]
fn insert_history_rows_pushes_viewport_down_when_screen_has_room() {
    let backend = TestBackend::with_lines(["view0 ", "view1 ", "      ", "      ", "      "]);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 0, 6, 2));

    let inserted = terminal
        .insert_history_rows(&history_rows_width(
            [Line::from("hist0"), Line::from("hist1")],
            6,
        ))
        .expect("insert history");

    assert_eq!(inserted, 2);
    assert_eq!(terminal.viewport_area(), Rect::new(0, 2, 6, 2));
    assert_eq!(terminal.visible_history_rows(), 2);
    terminal
        .backend()
        .assert_buffer_lines(["hist0 ", "hist1 ", "view0 ", "view1 ", "      "]);
}

#[test]
fn insert_history_rows_uses_synced_screen_size_when_moving_viewport() {
    let backend = TestBackend::with_lines(["view0   ", "view1   ", "        "]);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 0, 8, 2));
    terminal.backend_mut().resize(8, 5);
    terminal.sync_screen_size(Size::new(8, 5));

    terminal
        .insert_history_rows(&history_rows([Line::from("hist0"), Line::from("hist1")]))
        .expect("insert history");

    assert_eq!(terminal.last_known_screen_size(), Size::new(8, 5));
    assert_eq!(terminal.viewport_area(), Rect::new(0, 2, 8, 2));
}

#[test]
fn insert_history_rows_scrolls_overflow_into_scrollback() {
    let backend = TestBackend::with_lines(["old0 ", "view "]);
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 1, 5, 1));

    let inserted = terminal
        .insert_history_rows(&history_rows_width(
            [Line::from("hist0"), Line::from("hist1")],
            5,
        ))
        .expect("insert history");

    assert_eq!(inserted, 2);
    assert_eq!(terminal.visible_history_rows(), 1);
    terminal
        .backend()
        .assert_scrollback_lines(["old0 ", "hist0"]);
    terminal.backend().assert_buffer_lines(["hist1", "view "]);
}

#[test]
fn crossterm_surface_backend_purges_scrollback_and_screen_bytes() {
    let capture = CapturedWriter::default();
    let mut backend = CrosstermBackend::new(capture.clone());

    backend.clear_scrollback_and_screen().expect("clear bytes");

    let bytes = capture.ansi_bytes();
    parse_with_vt100(&bytes);
    assert!(
        bytes.starts_with("\x1b[r\x1b[0m\x1b[H"),
        "expected scroll-region/style reset and cursor home in {bytes:?}"
    );
    assert!(
        bytes.contains("\x1b[3J"),
        "expected scrollback purge in {bytes:?}"
    );
    assert!(
        bytes.contains("\x1b[2J"),
        "expected screen clear in {bytes:?}"
    );
}

#[test]
fn crossterm_surface_backend_emits_scroll_region_bytes() {
    let capture = CapturedWriter::default();
    let mut backend = CrosstermBackend::new(capture.clone());

    backend.scroll_region_up(0..3, 2).expect("scroll bytes");

    let bytes = capture.ansi_bytes();
    parse_with_vt100(&bytes);
    assert!(
        bytes.contains("\x1b[1;3r"),
        "expected DECSTBM scroll region in {bytes:?}"
    );
    assert!(
        bytes.contains("\x1b[2S"),
        "expected scroll-up command in {bytes:?}"
    );
    assert!(
        bytes.contains("\x1b[r"),
        "expected scroll region reset in {bytes:?}"
    );
}

#[test]
fn crossterm_surface_backend_leave_modes_omits_alt_screen_leave() {
    // Regression: the main session never enters the alternate screen, so the
    // exit mode-restore must NOT emit `LeaveAlternateScreen` (`CSI ?1049l`).
    // An unpaired `?1049l` does a DECRC onto whatever the shared save
    // register holds, yanking the cursor to a stale position so the resume
    // hint overprints the transcript. The modal-alt leave is a
    // separate, conditional step (`leave_modal_alt_screen`). codex's
    // `restore_common` omits the alt-screen leave for the same reason.
    let capture = CapturedWriter::default();
    let mut backend = CrosstermBackend::new(capture.clone());

    backend
        .leave_terminal_modes()
        .expect("leave terminal modes");

    let bytes = capture.ansi_bytes();
    parse_with_vt100(&bytes);
    assert!(
        !bytes.contains("\x1b[?1049l"),
        "exit mode-restore must not leave the alternate screen: {bytes:?}"
    );
    // Still tears down the input modes coco actually enabled.
    assert!(
        bytes.contains("\x1b[?2004l"),
        "expected bracketed-paste disable in {bytes:?}"
    );
    assert!(
        bytes.contains("\x1b[?1004l"),
        "expected focus-reporting disable in {bytes:?}"
    );
}

#[test]
fn crossterm_surface_backend_leave_modal_alt_screen_still_leaves_alt() {
    // The conditional modal-alt leave keeps emitting `?1049l` — that one is
    // paired with the `?1049h` from entering the modal, so it is correct.
    let capture = CapturedWriter::default();
    let mut backend = CrosstermBackend::new(capture.clone());

    backend
        .leave_modal_alt_screen()
        .expect("leave modal alt screen");

    let bytes = capture.ansi_bytes();
    parse_with_vt100(&bytes);
    assert!(
        bytes.contains("\x1b[?1049l"),
        "modal-alt leave must emit the alternate-screen leave: {bytes:?}"
    );
}

#[test]
fn crossterm_surface_backend_direct_inserts_plain_history_rows() {
    let capture = CapturedWriter::default();
    let backend = CrosstermBackend::new(capture.clone());
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 1, 8, 1));

    let rows = terminal
        .insert_history_rows(&history_rows([Line::from("plain")]))
        .expect("insert history");

    assert_eq!(rows, 1);
    let stats = terminal.last_history_insert_stats();
    assert_eq!(stats.buffer_updates, 0);
    assert!(stats.bytes_written > 0);
    let bytes = capture.ansi_bytes();
    parse_with_vt100(&bytes);
    assert!(
        bytes.contains("\x1b[2;1H\x1b[0mp"),
        "expected direct cursor-positioned write in {bytes:?}"
    );
    assert!(
        bytes.contains("\x1b[0m"),
        "expected style reset after direct write in {bytes:?}"
    );
}

#[test]
fn crossterm_surface_backend_direct_inserts_styled_and_wide_rows() {
    let capture = CapturedWriter::default();
    let backend = CrosstermBackend::new(capture.clone());
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 1, 8, 1));

    terminal
        .insert_history_rows(&history_rows([Line::from(vec![
            "界".red().bold(),
            " url".underlined(),
        ])]))
        .expect("insert history");

    let stats = terminal.last_history_insert_stats();
    assert_eq!(stats.buffer_updates, 0);
    assert!(stats.bytes_written > 0);
    let bytes = capture.ansi_bytes();
    parse_with_vt100(&bytes);
    assert!(bytes.contains("\x1b[0;31;1m界"), "{bytes:?}");
    assert!(bytes.contains("\x1b[0;4m url"), "{bytes:?}");
    assert!(bytes.contains("\x1b[0m"), "{bytes:?}");
}

#[test]
fn crossterm_surface_backend_direct_omits_wide_char_continuation_space() {
    // Regression: ratatui 0.30 fills a wide (CJK) char's continuation cell with
    // a reset space (`skip == false`), so the direct-insert path must skip it by
    // display width — otherwise `运动选择` is emitted as `运 动 选 择`.
    let capture = CapturedWriter::default();
    let backend = CrosstermBackend::new(capture.clone());
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 1, 12, 1));

    terminal
        .insert_history_rows(&history_rows_width([Line::from("运动选择")], 12))
        .expect("insert history");

    let bytes = capture.ansi_bytes();
    parse_with_vt100(&bytes);
    assert!(
        bytes.contains("运动选择"),
        "wide chars must be contiguous, no continuation spaces: {bytes:?}"
    );
}

#[test]
fn crossterm_surface_backend_direct_inserts_extended_modifiers() {
    let capture = CapturedWriter::default();
    let backend = CrosstermBackend::new(capture.clone());
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 1, 20, 1));

    terminal
        .insert_history_rows(&history_rows_width(
            [Line::from(vec![
                ratatui::text::Span::styled(
                    "gone",
                    Style::default().add_modifier(Modifier::CROSSED_OUT),
                ),
                ratatui::text::Span::styled(
                    " blink",
                    Style::default().add_modifier(Modifier::SLOW_BLINK | Modifier::HIDDEN),
                ),
                ratatui::text::Span::styled(
                    " loud",
                    Style::default()
                        .fg(ratatui::style::Color::LightRed)
                        .add_modifier(Modifier::BOLD | Modifier::RAPID_BLINK),
                ),
            ])],
            20,
        ))
        .expect("insert history");

    let bytes = capture.ansi_bytes();
    parse_with_vt100(&bytes);
    assert!(!bytes.contains("\x1b7"), "no DECSC: {bytes:?}");
    assert!(bytes.contains("\x1b[2;1H\x1b[0;9mgone"), "{bytes:?}");
    assert!(bytes.contains("\x1b[0;5;8m blink"), "{bytes:?}");
    assert!(bytes.contains("\x1b[0;91;1;6m loud"), "{bytes:?}");
    assert!(!bytes.contains("\x1b8"), "no DECRC: {bytes:?}");
    // SGR reset, then the absolute re-park at the tracked cursor (origin —
    // nothing has claimed the cursor yet).
    assert!(bytes.ends_with("\x1b[0m\x1b[1;1H"), "{bytes:?}");
}

#[test]
fn crossterm_surface_backend_insert_reparks_cursor_at_tracked_claim() {
    // The insert must end with an absolute move back to the position the
    // last frame's cursor claim parked — app-owned bookkeeping, not the
    // terminal's DECSC/DECRC save register.
    let capture = CapturedWriter::default();
    let backend = CrosstermBackend::new(capture.clone());
    let mut terminal = SurfaceTerminal::new(backend).expect("terminal");
    terminal.set_viewport_area(Rect::new(0, 1, 20, 1));
    terminal
        .draw_viewport(|frame| {
            frame.set_cursor_claim(CursorClaim {
                position: Position { x: 3, y: 1 },
                style: SetCursorStyle::SteadyBar,
            });
        })
        .expect("draw viewport");

    terminal
        .insert_history_rows(&history_rows_width([Line::from("hello")], 20))
        .expect("insert history");

    let bytes = capture.ansi_bytes();
    parse_with_vt100(&bytes);
    assert!(!bytes.contains("\x1b7"), "no DECSC: {bytes:?}");
    assert!(!bytes.contains("\x1b8"), "no DECRC: {bytes:?}");
    assert!(
        bytes.ends_with("\x1b[2;4H"),
        "expected absolute re-park at the claimed cursor: {bytes:?}"
    );
}

#[test]
fn crossterm_surface_backend_emits_cursor_style_and_sync_update_bytes() {
    let capture = CapturedWriter::default();
    let mut backend = CrosstermBackend::new(capture.clone());

    backend
        .begin_synchronized_update()
        .expect("begin sync bytes");
    backend
        .set_cursor_style(SetCursorStyle::SteadyBar)
        .expect("cursor style bytes");
    backend.end_synchronized_update().expect("end sync bytes");

    let bytes = capture.ansi_bytes();
    parse_with_vt100(&bytes);
    assert!(
        bytes.contains("\x1b[?2026h"),
        "expected begin synchronized update in {bytes:?}"
    );
    assert!(
        bytes.contains("\x1b[6 q"),
        "expected steady bar cursor style in {bytes:?}"
    );
    assert!(
        bytes.contains("\x1b[?2026l"),
        "expected end synchronized update in {bytes:?}"
    );
}

#[test]
fn clear_after_position_stays_inside_synchronized_window() {
    // The per-frame draw path must emit the viewport clear AFTER `?2026h` and
    // BEFORE `?2026l`, so a terminal supporting synchronized update never
    // presents the cleared (blank) region before the repaint — the fix for the
    // streaming input-bar flicker.
    let capture = CapturedWriter::default();
    let backend = CrosstermBackend::new(capture.clone());
    let mut terminal = SurfaceTerminal::new(backend).expect("test backend is infallible");
    terminal.set_viewport_area(Rect::new(0, 4, 40, 6));
    capture.reset();

    terminal.begin_synchronized_update().expect("begin sync");
    terminal
        .clear_after_position(Position { x: 0, y: 2 })
        .expect("clear queues");
    terminal.end_synchronized_update().expect("end sync");

    let bytes = capture.ansi_bytes();
    let begin = bytes.find("\x1b[?2026h").expect("begin sync present");
    let end = bytes.find("\x1b[?2026l").expect("end sync present");
    let clear = bytes
        .find("\x1b[0J")
        .or_else(|| bytes.find("\x1b[J"))
        .expect("clear-to-end present");
    assert!(begin < clear, "clear must follow ?2026h: {bytes:?}");
    assert!(clear < end, "clear must precede ?2026l: {bytes:?}");
}

fn parse_with_vt100(bytes: &str) {
    let mut parser = vt100::Parser::new(8, 16, 16);
    parser.process(bytes.as_bytes());
}

#[derive(Debug, Default, Clone)]
struct CapturedWriter {
    bytes: Rc<RefCell<Vec<u8>>>,
}

impl CapturedWriter {
    fn ansi_bytes(&self) -> String {
        String::from_utf8(self.bytes.borrow().clone()).expect("crossterm bytes are utf8")
    }

    /// Drop setup bytes emitted by terminal construction so an assertion
    /// observes only the operations under test.
    fn reset(&self) {
        self.bytes.borrow_mut().clear();
    }
}

impl Write for CapturedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.bytes.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
