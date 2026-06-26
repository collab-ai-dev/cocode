//! Pure, unit-tested metric predicates for the render benchmarks.
//!
//! Ported from opencode's perf-probe trust chain: the math that turns raw
//! counters into a pass/fail signal lives in pure functions with their own
//! unit tests, and the bench recipe (`just bench`) runs those tests BEFORE any
//! benchmark executes (opencode's `bun test ./unit && playwright`). A bench
//! assertion (`assert!(is_clean_cache_hit(..))`) is then only as trustworthy
//! as a *tested* function, not ad-hoc inline arithmetic that no one verifies.
//!
//! Gated behind `testing` so it never ships in release; `app/tui` enables that
//! feature on its `coco-tui-ui` dev-dependency, so both crates' benches can
//! call these.

/// A replay is a "clean cache hit" iff the cache served it AND no finalized
/// render was recomputed. Both conditions matter: a cache hit that still
/// re-renders defeats the point of the cache, so the bench must assert both.
pub fn is_clean_cache_hit(cache_hit: bool, finalized_render_calls: usize) -> bool {
    cache_hit && finalized_render_calls == 0
}

/// Fraction of painted cells that actually changed, in `0.0..=1.0`. Returns
/// `0.0` for an empty surface (nothing painted ⇒ nothing wasted). This is the
/// cell-diff effectiveness metric: a redraw of unchanged content should trend
/// toward `0.0`.
pub fn changed_cell_ratio(changed_cells: usize, total_cells: usize) -> f64 {
    if total_cells == 0 {
        0.0
    } else {
        changed_cells as f64 / total_cells as f64
    }
}

/// A repaint of identical content is "minimal" iff the cell-diff found nothing
/// to redraw — the flicker/efficiency invariant for an unchanged frame.
pub fn is_minimal_repaint(changed_cells: usize) -> bool {
    changed_cells == 0
}

#[cfg(test)]
#[path = "bench_metrics.test.rs"]
mod tests;
