//! Native normal-path surface benchmark.
//!
//! Run:
//! `cargo bench -p coco-tui --features testing --bench native_surface_normal -- --quick`

use coco_tui::testing::NativeSurfaceNormalBench;
use coco_tui::testing::NativeSurfaceNormalBenchContent;
use criterion::BatchSize;
use criterion::BenchmarkId;
use criterion::Criterion;
use criterion::black_box;
use criterion::criterion_group;
use criterion::criterion_main;

fn bench_native_surface_normal(c: &mut Criterion) {
    let mut group = c.benchmark_group("native_surface_normal");
    let turns = 500;
    let width = 100;
    let height = 40;

    group.bench_function("large_scrollback_streaming_no_transcript_change", |b| {
        let mut bench = NativeSurfaceNormalBench::new(
            turns,
            width,
            height,
            NativeSurfaceNormalBenchContent::Markdown,
            true,
        );
        b.iter(|| black_box(bench.redraw_no_transcript_change()));
    });

    for content in [
        NativeSurfaceNormalBenchContent::StreamNoNewline,
        NativeSurfaceNormalBenchContent::StreamNewlineHeavy,
        NativeSurfaceNormalBenchContent::StreamTable,
    ] {
        group.bench_with_input(
            BenchmarkId::new(
                "large_stream_redraw_no_transcript_change",
                format!("{content:?}"),
            ),
            &content,
            |b, &content| {
                let mut bench = NativeSurfaceNormalBench::new(turns, width, height, content, true);
                b.iter(|| black_box(bench.redraw_no_transcript_change()));
            },
        );
    }

    group.bench_function("large_scrollback_input_animation", |b| {
        let mut bench = NativeSurfaceNormalBench::new(
            turns,
            width,
            height,
            NativeSurfaceNormalBenchContent::Markdown,
            false,
        );
        b.iter(|| black_box(bench.redraw_after_input_animation()));
    });

    group.bench_function("large_scrollback_one_committed_append", |b| {
        b.iter_batched(
            || {
                NativeSurfaceNormalBench::new(
                    turns,
                    width,
                    height,
                    NativeSurfaceNormalBenchContent::Markdown,
                    false,
                )
            },
            |mut bench| {
                black_box(
                    bench.append_one_committed_message(NativeSurfaceNormalBenchContent::Markdown),
                )
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("direct_insert_plain_stable_lines", |b| {
        b.iter_batched(
            || {
                NativeSurfaceNormalBench::new(
                    turns,
                    width,
                    height,
                    NativeSurfaceNormalBenchContent::Markdown,
                    false,
                )
            },
            |mut bench| {
                black_box(bench.start_streaming_message(NativeSurfaceNormalBenchContent::Plain))
            },
            BatchSize::SmallInput,
        );
    });

    // Regression sentinel for the app-level finalization path. This uses
    // ratatui's TestBackend and is not a production direct-VT latency metric.
    group.bench_function("test_backend_stream_final_consolidation_regression", |b| {
        b.iter_batched(
            || {
                let mut bench = NativeSurfaceNormalBench::new(
                    turns,
                    width,
                    height,
                    NativeSurfaceNormalBenchContent::Markdown,
                    false,
                );
                bench.start_streaming_message(NativeSurfaceNormalBenchContent::StreamNewlineHeavy);
                bench
            },
            |mut bench| {
                black_box(bench.finalize_streaming_message(
                    NativeSurfaceNormalBenchContent::StreamNewlineHeavy,
                ))
            },
            BatchSize::SmallInput,
        );
    });

    for (label, content, streaming) in [
        (
            "syntax_enabled_streaming_fenced_code",
            NativeSurfaceNormalBenchContent::SyntaxCode,
            true,
        ),
        (
            "streaming_mermaid_suppressed_finalized_append",
            NativeSurfaceNormalBenchContent::Mermaid,
            false,
        ),
    ] {
        group.bench_with_input(BenchmarkId::new(label, turns), &content, |b, &content| {
            b.iter_batched(
                || NativeSurfaceNormalBench::new(turns, width, height, content, streaming),
                |mut bench| black_box(bench.append_one_committed_message(content)),
                BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

criterion_group!(benches, bench_native_surface_normal);
criterion_main!(benches);
