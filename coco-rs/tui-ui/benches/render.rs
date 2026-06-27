//! Headless render micro-benchmarks for the pure presentational layer.
//!
//! Modeled on jcode's `tui_bench`: drive the **real** render primitives — the
//! native-scrollback paint engine (`SurfaceTerminal` over ratatui's
//! `TestBackend`) and the markdown renderer — in a tight loop and report timing
//! percentiles (criterion). Each group maps to a deferred optimization so the
//! "profile first" decision becomes a measurable, CI-checkable signal:
//!
//! - `markdown_parse` — cost of parsing markdown → `Vec<Line>`, by document
//!   size. The input to every wrap/paint and a regression guard on the parser.
//!   (Width is *not* a parameter here: `render_markdown` wraps nothing — it
//!   only sizes code-fence borders + table columns; line wrapping happens later
//!   at paint time, which is what `reflow_wrap` measures.)
//! - `reflow_wrap` — width-driven wrap+paint through the real engine
//!   (`Paragraph::wrap` → `draw_viewport`) across terminal widths. The
//!   per-resize/replay re-wrap cost a wrapped-line cache (deferred R1) avoids.
//! - `surface_paint` — cell-diff effectiveness: a cold `first_frame` (full
//!   paint) vs `repaint_unchanged` (identical content → the diff finds ~no
//!   changed cells). The gap is the per-tick win the engine already delivers
//!   and the floor a spinner fast-path would build on.
//! - `markdown_streaming` — re-parse cost of a growing live cell: every frame
//!   re-parses the visible buffer today (no incremental cache). A future
//!   incremental-markdown checkpoint should shrink this group for the same
//!   input.
//!
//! Run: `cargo bench -p coco-tui-ui --bench render`. Fast smoke: append `-- --quick`.
//! (Target `--bench render` explicitly — a bare `cargo bench` also runs the lib
//! unit tests through libtest, which rejects criterion flags like `--quick`.)

use std::hint::black_box;
use std::io;
use std::io::Write;

use criterion::BatchSize;
use criterion::BenchmarkId;
use criterion::Criterion;
use criterion::criterion_group;
use criterion::criterion_main;
use ratatui::backend::CrosstermBackend;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;

use coco_tui_markdown::MarkdownOptions;
use coco_tui_markdown::render_markdown;
use coco_tui_ui::display::SyntaxHighlighting;
use coco_tui_ui::engine::history_insert::render_history_rows;
use coco_tui_ui::engine::terminal::SurfaceFrame;
use coco_tui_ui::engine::terminal::SurfaceTerminal;
use coco_tui_ui::style::UiStyles;
use coco_tui_ui::theme::Theme;

/// One representative assistant message block: prose + fenced code + list + table.
const BLOCK: &str = "\
Here is a summary of the change and why it matters for the render path.

```rust
fn quantize(r: u8, g: u8, b: u8) -> u8 {
    // weighted nearest-cube vs grayscale ramp
    nearest_xterm256(r, g, b)
}
```

- first point about the wrapped-line cache
- second point about native scrollback and zero retained RAM
- third point that wraps past the terminal width to exercise reflow logic here

| column a | column b | column c |
|----------|----------|----------|
| value 1  | value 2  | value 3  |
";

/// Build a markdown document of `blocks` repeated message blocks.
fn sample_doc(blocks: usize) -> String {
    BLOCK.repeat(blocks)
}

/// Render `doc` to styled lines at `width` (syntax highlighting on).
fn render_lines(doc: &str, theme: &Theme, width: u16) -> Vec<Line<'static>> {
    render_markdown(
        doc,
        MarkdownOptions::new(UiStyles::new(theme), width, SyntaxHighlighting::Enabled),
        None,
    )
}

/// A `SurfaceTerminal<TestBackend>` sized to `width`×`height` with its viewport
/// spanning the whole screen — the real paint target the shell drives.
fn surface(width: u16, height: u16) -> SurfaceTerminal<TestBackend> {
    let mut term = SurfaceTerminal::new(TestBackend::new(width, height)).expect("surface terminal");
    term.set_viewport_area(Rect::new(0, 0, width, height));
    term
}

fn crossterm_surface(width: u16, height: u16) -> SurfaceTerminal<CrosstermBackend<CountingWriter>> {
    let mut term = SurfaceTerminal::new(CrosstermBackend::new(CountingWriter::default()))
        .expect("surface terminal");
    term.set_viewport_area(Rect::new(0, height.saturating_sub(12), width, 12));
    term
}

#[derive(Debug, Default)]
struct CountingWriter {
    bytes: usize,
}

impl Write for CountingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.bytes = self.bytes.saturating_add(buf.len());
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Markdown parse cost by document size — the input to every wrap/paint.
fn bench_markdown_parse(c: &mut Criterion) {
    let theme = Theme::default();
    let mut group = c.benchmark_group("markdown_parse");
    for blocks in [8usize, 20] {
        let doc = sample_doc(blocks);
        group.bench_with_input(BenchmarkId::from_parameter(blocks), &doc, |b, doc| {
            b.iter(|| black_box(render_lines(black_box(doc), &theme, 100)));
        });
    }
    group.finish();
}

/// Width-driven wrap+paint through the real engine — the per-resize re-wrap
/// cost a wrapped-line cache (deferred R1) would avoid re-doing.
fn bench_reflow_wrap(c: &mut Criterion) {
    let theme = Theme::default();
    let height = 50u16;
    let mut group = c.benchmark_group("reflow_wrap");
    for width in [40u16, 80, 120, 160] {
        let para =
            Paragraph::new(render_lines(&sample_doc(20), &theme, width)).wrap(Wrap { trim: false });
        let mut term = surface(width, height);
        // Warm the diff buffers so we measure steady-state wrap+paint, not the
        // one-off cold full paint (which `surface_paint/first_frame` isolates).
        term.draw_viewport(|f: &mut SurfaceFrame| f.render_widget(&para, f.area()))
            .expect("warm");
        group.bench_with_input(BenchmarkId::from_parameter(width), &width, |b, _| {
            b.iter(|| {
                term.draw_viewport(|f: &mut SurfaceFrame| {
                    f.render_widget(black_box(&para), f.area())
                })
                .expect("draw");
            });
        });
    }
    group.finish();
}

/// Cell-diff effectiveness: cold full paint vs identical-content repaint.
fn bench_surface_paint(c: &mut Criterion) {
    let theme = Theme::default();
    let (width, height) = (100u16, 40u16);
    let para =
        Paragraph::new(render_lines(&sample_doc(8), &theme, width)).wrap(Wrap { trim: false });

    let mut group = c.benchmark_group("surface_paint");

    // Cold: a fresh terminal each iter → the diff repaints every cell.
    group.bench_function("first_frame", |b| {
        b.iter_batched(
            || surface(width, height),
            |mut term| {
                term.draw_viewport(|f: &mut SurfaceFrame| f.render_widget(&para, f.area()))
                    .expect("draw");
            },
            BatchSize::SmallInput,
        );
    });

    // Warm: identical content repainted → the diff finds ~no changed cells.
    let mut term = surface(width, height);
    term.draw_viewport(|f: &mut SurfaceFrame| f.render_widget(&para, f.area()))
        .expect("warm");
    group.bench_function("repaint_unchanged", |b| {
        b.iter(|| {
            term.draw_viewport(|f: &mut SurfaceFrame| f.render_widget(&para, f.area()))
                .expect("draw");
        });
    });
    group.finish();
}

/// Re-parse cost of a live streaming cell: render growing prefixes at a fixed
/// width (each frame re-parses the visible buffer — no incremental cache today).
fn bench_markdown_streaming(c: &mut Criterion) {
    let theme = Theme::default();
    let full = sample_doc(12);
    let width = 100u16;
    const FRAMES: usize = 30;

    // Char-boundary-aligned growing prefixes simulating per-frame streaming.
    let prefixes: Vec<&str> = (1..=FRAMES)
        .map(|i| {
            let mut end = (full.len() * i / FRAMES).min(full.len());
            while end < full.len() && !full.is_char_boundary(end) {
                end += 1;
            }
            &full[..end]
        })
        .collect();

    let mut group = c.benchmark_group("markdown_streaming");
    group.bench_function("grow_to_full", |b| {
        b.iter(|| {
            for prefix in &prefixes {
                black_box(render_lines(black_box(prefix), &theme, width));
            }
        });
    });
    group.finish();
}

fn bench_history_insert(c: &mut Criterion) {
    let theme = Theme::default();
    let width = 100u16;
    let height = 40u16;
    let lines = render_lines("plain\n**styled**\n界 url", &theme, width);
    let final_remainder_lines =
        render_lines("\nfinal remainder after provisional prefix", &theme, width);

    let mut group = c.benchmark_group("history_insert");
    group.bench_function("test_backend_large_scrollback", |b| {
        b.iter_batched(
            || {
                let mut term = surface(width, height);
                term.set_viewport_area(Rect::new(0, height.saturating_sub(12), width, 12));
                for i in 0..500 {
                    let rows = render_history_rows(vec![Line::from(format!("seed {i}"))], width);
                    term.insert_history_rows(&rows).expect("seed");
                }
                term
            },
            |mut term| {
                let rows = render_history_rows(lines.clone(), width);
                black_box(term.insert_history_rows(&rows).expect("insert"));
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("crossterm_direct_vt", |b| {
        b.iter_batched(
            || crossterm_surface(width, height),
            |mut term| {
                let rows = render_history_rows(lines.clone(), width);
                black_box(term.insert_history_rows(&rows).expect("insert"));
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("crossterm_direct_vt_final_remainder", |b| {
        b.iter_batched(
            || crossterm_surface(width, height),
            |mut term| {
                let rows = render_history_rows(final_remainder_lines.clone(), width);
                black_box(term.insert_history_rows(&rows).expect("insert"));
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_markdown_parse,
    bench_reflow_wrap,
    bench_surface_paint,
    bench_markdown_streaming,
    bench_history_insert
);
criterion_main!(benches);
