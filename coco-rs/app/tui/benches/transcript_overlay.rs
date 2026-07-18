//! Ctrl+O transcript-reader layout benchmark (plan item B7).
//!
//! Paired variants make plan item B3 measurable and guard it. The reader's
//! per-cell height cache used to be flushed on every content-generation bump —
//! and the generation moves on any transcript or tool-status change — so with
//! the reader open during a turn, laying out the pager re-rendered every cell
//! in the history on every change: O(history) full cell renders per delta.
//!
//! `reset_index` reproduces that flush; `retained_index` is what ships. The gap
//! between the two IS the fix, and it should widen with transcript size.
//!
//! Run: `cargo bench -p coco-tui --features testing --bench transcript_overlay`.
//! Fast smoke: append `-- --quick`.

use std::hint::black_box;

use coco_tui::testing::TranscriptOverlayBench;
use criterion::BenchmarkId;
use criterion::Criterion;
use criterion::criterion_group;
use criterion::criterion_main;

/// Transcript sizes in turns. Each turn derives roughly three cells (user +
/// assistant-with-tool-call + tool result), so 1000 turns ≈ the ~3k-cell
/// transcript a long session reaches.
const TURN_COUNTS: [usize; 2] = [200, 1000];
const WIDTH: u16 = 100;
const HEIGHT: u16 = 40;

fn bench_transcript_overlay(c: &mut Criterion) {
    let mut group = c.benchmark_group("transcript_overlay");
    for turns in TURN_COUNTS {
        let cells = TranscriptOverlayBench::new(turns).cell_count();

        // Steady-state layout with the cache retained: only cells whose key
        // actually changed are re-measured.
        group.bench_with_input(
            BenchmarkId::new("retained_index", cells),
            &turns,
            |b, &turns| {
                let mut bench = TranscriptOverlayBench::new(turns);
                // Warm the cache so the measurement is steady state, not first paint.
                bench.render_with_retained_index(WIDTH, HEIGHT);
                b.iter(|| black_box(bench.render_with_retained_index(WIDTH, HEIGHT)));
            },
        );

        // The pre-B3 shape: every frame re-measures the whole history.
        group.bench_with_input(
            BenchmarkId::new("reset_index", cells),
            &turns,
            |b, &turns| {
                let mut bench = TranscriptOverlayBench::new(turns);
                bench.render_with_reset_index(WIDTH, HEIGHT);
                b.iter(|| black_box(bench.render_with_reset_index(WIDTH, HEIGHT)));
            },
        );

        // The live case B3 is really about: the reader open while a turn
        // appends. Every append bumps the generation, which is exactly what
        // used to flush the cache.
        group.bench_with_input(
            BenchmarkId::new("retained_index_streaming_append", cells),
            &turns,
            |b, &turns| {
                let mut bench = TranscriptOverlayBench::new(turns);
                bench.render_with_retained_index(WIDTH, HEIGHT);
                let mut index = 0usize;
                b.iter(|| {
                    bench.append_turn(index);
                    index += 1;
                    black_box(bench.render_with_retained_index(WIDTH, HEIGHT))
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_transcript_overlay);
criterion_main!(benches);
