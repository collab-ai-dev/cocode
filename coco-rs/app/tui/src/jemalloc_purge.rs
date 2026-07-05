//! End-of-turn jemalloc arena purge.
//!
//! macOS jemalloc builds have no `background_thread` (it's excluded for the
//! macho ABI), so freed pages only decay lazily on allocation traffic — an
//! idle TUI between turns never reclaims them, and physical footprint drifts
//! up across a session even though the retained application data is flat. At
//! every `TurnEnded` we run an explicit `arena.*.purge` (a short MADV_DONTNEED
//! sweep) on a blocking thread, off the render loop, and log the resident
//! delta so `/coco-analyze --mem` can attribute the drop.
//!
//! Compiled to a no-op when the `jemalloc` feature is off — the wrapper crate's
//! [`ENABLED`](coco_utils_jemalloc::ENABLED) short-circuits before any task is
//! spawned.

use coco_utils_jemalloc::JemallocStats;

/// Spawn the end-of-turn purge as a detached blocking task. Cheap to call on
/// every turn: returns immediately when jemalloc control isn't compiled in, so
/// the common (non-jemalloc) build never spawns anything.
pub(crate) fn spawn_turn_ended_purge() {
    if !coco_utils_jemalloc::ENABLED {
        return;
    }
    // Purge + the two stat reads are a handful of syscalls (the MADV_DONTNEED
    // sweep dominates and can run into low-single-digit ms on a large dirty
    // set), so keep them off the UI thread. Fire-and-forget: the task borrows
    // nothing and only logs.
    tokio::task::spawn_blocking(|| {
        let before = coco_utils_jemalloc::stats_snapshot();
        match coco_utils_jemalloc::purge_all_arenas() {
            Ok(()) => log_purge(before, coco_utils_jemalloc::stats_snapshot()),
            Err(err) => {
                tracing::warn!(target: "tui::perf::mem", %err, "jemalloc arena purge failed");
            }
        }
    });
}

fn log_purge(before: Option<JemallocStats>, after: Option<JemallocStats>) {
    let (Some(before), Some(after)) = (before, after) else {
        tracing::debug!(
            target: "tui::perf::mem",
            "jemalloc arena purge (turn_ended); stats unavailable"
        );
        return;
    };
    tracing::debug!(
        target: "tui::perf::mem",
        resident_before_bytes = before.resident,
        resident_after_bytes = after.resident,
        resident_reclaimed_bytes = before.resident.saturating_sub(after.resident),
        retained_before_bytes = before.retained,
        retained_after_bytes = after.retained,
        allocated_bytes = after.allocated,
        active_bytes = after.active,
        "jemalloc arena purge (turn_ended)"
    );
}
