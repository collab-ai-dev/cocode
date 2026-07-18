//! Actual `StreamRenderController` growth benchmark (plan item B2).
//!
//! Run: `cargo bench -p coco-tui --features testing --bench streaming_markdown`.
//! Fast smoke: append `-- --quick`.

use coco_tui::testing::StreamingMarkdownBench;
use criterion::BatchSize;
use criterion::BenchmarkId;
use criterion::Criterion;
use criterion::black_box;
use criterion::criterion_group;
use criterion::criterion_main;

fn bench_streaming_markdown(c: &mut Criterion) {
    let mut group = c.benchmark_group("streaming_markdown_controller");
    for blocks in [50_usize, 500] {
        group.bench_with_input(
            BenchmarkId::new("append_only_growth", blocks),
            &blocks,
            |b, &blocks| {
                b.iter_batched(
                    || StreamingMarkdownBench::new(100),
                    |mut bench| black_box(bench.grow_blocks(blocks)),
                    BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_streaming_markdown);
criterion_main!(benches);
