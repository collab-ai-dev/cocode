//! Native finalized-history replay benchmark.
//!
//! Run: `cargo bench -p coco-tui --features testing --bench native_replay`.
//! Fast smoke: append `-- --quick`.

use std::hint::black_box;

use coco_tui::testing::NativeReplayBench;
use coco_tui::testing::NativeReplayBenchContent;
use coco_tui::testing::clear_native_replay_markdown_memo;
use criterion::BatchSize;
use criterion::BenchmarkId;
use criterion::Criterion;
use criterion::criterion_group;
use criterion::criterion_main;

fn bench_native_replay(c: &mut Criterion) {
    let mut group = c.benchmark_group("native_replay");
    for width in [80_u16, 120] {
        bench_render_case(
            &mut group,
            "markdown",
            NativeReplayBenchContent::Markdown,
            width,
            MemoMode::Cold,
        );
        bench_render_case(
            &mut group,
            "markdown",
            NativeReplayBenchContent::Markdown,
            width,
            MemoMode::Warm,
        );
        bench_cached_render_case(
            &mut group,
            "markdown",
            NativeReplayBenchContent::Markdown,
            width,
        );
        bench_insert_case(
            &mut group,
            "markdown",
            NativeReplayBenchContent::Markdown,
            width,
            MemoMode::Cold,
        );
        bench_insert_case(
            &mut group,
            "markdown",
            NativeReplayBenchContent::Markdown,
            width,
            MemoMode::Warm,
        );
        bench_cached_insert_case(
            &mut group,
            "markdown",
            NativeReplayBenchContent::Markdown,
            width,
        );

        for (label, content) in [
            ("syntax", NativeReplayBenchContent::SyntaxCode),
            ("mermaid", NativeReplayBenchContent::Mermaid),
            // The dominant production shape (thinking + text + tool call +
            // result per turn). Its cached cases double as a structural
            // guard: `bench_cached_render_case` asserts `cache_hit`, so a
            // change that makes tool-bearing transcripts uncacheable again
            // fails the bench instead of silently regressing every
            // resize/theme replay.
            ("tool_heavy", NativeReplayBenchContent::ToolHeavy),
        ] {
            bench_render_case(&mut group, label, content, width, MemoMode::Warm);
            bench_cached_render_case(&mut group, label, content, width);
            bench_insert_case(&mut group, label, content, width, MemoMode::Warm);
            bench_cached_insert_case(&mut group, label, content, width);
        }
    }
    group.finish();
}

#[derive(Debug, Clone, Copy)]
enum MemoMode {
    Cold,
    Warm,
}

fn bench_render_case(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    label: &str,
    content: NativeReplayBenchContent,
    width: u16,
    memo_mode: MemoMode,
) {
    let id = BenchmarkId::new(
        format!("render_uncached_{label}_{memo_mode:?}").to_lowercase(),
        width,
    );
    group.bench_with_input(id, &width, |b, &width| {
        b.iter_batched(
            || {
                if matches!(memo_mode, MemoMode::Cold) {
                    clear_native_replay_markdown_memo();
                }
                let bench = NativeReplayBench::new_with_content(turns_for(content), content);
                if matches!(memo_mode, MemoMode::Warm) {
                    let _ = bench.render_uncached(width);
                }
                bench
            },
            |bench| black_box(bench.render_uncached(width)),
            BatchSize::SmallInput,
        );
    });
}

fn bench_cached_render_case(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    label: &str,
    content: NativeReplayBenchContent,
    width: u16,
) {
    group.bench_with_input(
        BenchmarkId::new(format!("render_cached_hit_{label}"), width),
        &width,
        |b, &width| {
            let mut bench = NativeReplayBench::new_with_content(turns_for(content), content);
            let warmed = bench.render_cached(width);
            assert!(!warmed.cache_hit);
            b.iter(|| {
                let output = bench.render_cached(width);
                assert!(output.cache_hit);
                assert_eq!(output.finalized_render_calls, 0);
                black_box(output.lines)
            });
        },
    );
}

fn bench_insert_case(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    label: &str,
    content: NativeReplayBenchContent,
    width: u16,
    memo_mode: MemoMode,
) {
    let id = BenchmarkId::new(
        format!("insert_uncached_{label}_{memo_mode:?}").to_lowercase(),
        width,
    );
    group.bench_with_input(id, &width, |b, &width| {
        b.iter_batched(
            || {
                if matches!(memo_mode, MemoMode::Cold) {
                    clear_native_replay_markdown_memo();
                }
                let bench = NativeReplayBench::new_with_content(turns_for(content), content);
                if matches!(memo_mode, MemoMode::Warm) {
                    let _ = bench.render_uncached(width);
                }
                bench
            },
            |bench| black_box(bench.insert_uncached(width, 40)),
            BatchSize::SmallInput,
        );
    });
}

fn bench_cached_insert_case(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    label: &str,
    content: NativeReplayBenchContent,
    width: u16,
) {
    group.bench_with_input(
        BenchmarkId::new(format!("insert_cached_hit_{label}"), width),
        &width,
        |b, &width| {
            let mut bench = NativeReplayBench::new_with_content(turns_for(content), content);
            let warmed = bench.insert_cached(width, 40);
            assert!(!warmed.cache_hit);
            b.iter(|| {
                let output = bench.insert_cached(width, 40);
                assert!(output.cache_hit);
                assert_eq!(output.finalized_render_calls, 0);
                black_box(output.rows)
            });
        },
    );
}

fn turns_for(content: NativeReplayBenchContent) -> usize {
    match content {
        NativeReplayBenchContent::Markdown | NativeReplayBenchContent::SyntaxCode => 200,
        NativeReplayBenchContent::Mermaid => 80,
        // Four cells per turn (user/thinking+text+tool/result), so fewer
        // turns keep the replay within the row cap.
        NativeReplayBenchContent::ToolHeavy => 120,
    }
}

criterion_group!(benches, bench_native_replay);
criterion_main!(benches);
