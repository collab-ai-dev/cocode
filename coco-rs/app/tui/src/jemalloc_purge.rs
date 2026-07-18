//! Memory-cliff jemalloc arena purge + optional heap-profile dumps.
//!
//! macOS jemalloc builds have no `background_thread` (it's excluded for the
//! macho ABI), so freed pages only decay lazily on allocation traffic — an
//! idle TUI never reclaims them, and physical footprint drifts up across a
//! session even though the retained application data is flat. At every memory
//! cliff (see [`crate::perf::MemoryPhase`]) we run an explicit `arena.*.purge`
//! (a short MADV_DONTNEED sweep) on a blocking thread, off the render loop, and
//! log the resident delta so `/coco-analyze --mem` can attribute the drop.
//!
//! Turn end is the obvious cliff, but not the only one: a session resume,
//! `/clear`, a rewind, or a compaction each free a large replay/history graph
//! at once. Purging only at turn end left those pages resident until the *next*
//! turn ended — which, right after a resume, may be minutes away or never. Each
//! purge carries the `reason` that triggered it so per-site effectiveness is
//! visible in the log rather than guessed at.
//!
//! When `tui.performance.heap_profile_enabled` is set (and the process was
//! started with `prof:true` — `just coco-jemalloc` arranges that), the same
//! turn boundary also writes a `prof.dump` heap profile, so allocated-bytes
//! growth between two turns can be attributed to call stacks with `jeprof` /
//! `jemalloc-pprof` instead of log bisection.
//!
//! Compiled to a no-op when the `jemalloc` feature is off — the wrapper crate's
//! [`ENABLED`](coco_utils_jemalloc::ENABLED) short-circuits before any task is
//! spawned.

use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use coco_utils_jemalloc::JemallocStats;

/// Monotonic sequence for heap-profile dump filenames within this process.
/// Each `TurnEnded` dump takes the next value, so consecutive dumps diff as
/// "what turn N retained".
static HEAP_DUMP_SEQ: AtomicU64 = AtomicU64::new(0);
/// Last state pushed via [`sync_heap_profiling`], so settings hot-reloads only
/// touch `prof.active` (and log) when the desired state actually changes.
static HEAP_PROFILING_DESIRED: AtomicBool = AtomicBool::new(false);

/// Spawn a purge (and, when enabled, a heap-profile dump) as a detached
/// blocking task, attributed to the cliff that triggered it.
///
/// The triggering [`crate::perf::MemoryPhase`] lands as a structured log field
/// (via its `as_str`) so each site's reclaim can be measured separately. Cheap
/// to call: returns immediately when jemalloc control isn't compiled in, so the
/// common (non-jemalloc) build records an ordered unavailable event without
/// touching allocator controls.
pub(crate) fn spawn_purge(
    phase: crate::perf::MemoryPhase,
    heap_profile_enabled: bool,
    trace_job: crate::memory_trace::MemoryTracePurgeJob,
) {
    let reason = phase.as_str();
    // Purge + the two stat reads are a handful of syscalls (the MADV_DONTNEED
    // sweep dominates and can run into low-single-digit ms on a large dirty
    // set), so keep them off the UI thread. Fire-and-forget: the task borrows
    // nothing. The trace job's ticket also serializes the allocator mutation
    // after its pre-purge sample, preserving both measurement and JSONL order.
    tokio::task::spawn_blocking(move || {
        trace_job.run(|memory_trace| {
            if !coco_utils_jemalloc::ENABLED {
                memory_trace.record_purge(
                    phase,
                    None,
                    None,
                    std::time::Duration::ZERO,
                    Some("jemalloc_unavailable"),
                );
                return;
            }

            let started = std::time::Instant::now();
            let before = coco_utils_jemalloc::stats_snapshot();
            match coco_utils_jemalloc::purge_all_arenas() {
                Ok(()) => {
                    let after = coco_utils_jemalloc::stats_snapshot();
                    log_purge(reason, before, after);
                    memory_trace.record_purge(phase, before, after, started.elapsed(), None);
                }
                Err(err) => {
                    let error = err.to_string();
                    tracing::warn!(
                        target: "tui::perf::mem",
                        %err,
                        reason,
                        "jemalloc arena purge failed"
                    );
                    memory_trace.record_purge(
                        phase,
                        before,
                        coco_utils_jemalloc::stats_snapshot(),
                        started.elapsed(),
                        Some(&error),
                    );
                }
            }
        });
        if coco_utils_jemalloc::ENABLED && heap_profile_enabled {
            dump_heap_profile(reason);
        }
    });
}

/// Push the desired `tui.performance.heap_profile_enabled` state into
/// jemalloc's `prof.active` sampling gate. Called at startup and on every
/// display-settings hot-reload; no-ops unless the desired state changed.
///
/// Activation only takes effect when the process started with `prof:true`
/// (jemalloc fixes `opt.prof` at startup); otherwise a WARN explains how to
/// get a profiling-capable run.
pub(crate) fn sync_heap_profiling(enabled: bool) {
    if HEAP_PROFILING_DESIRED.swap(enabled, Ordering::Relaxed) == enabled {
        return;
    }
    if !coco_utils_jemalloc::ENABLED {
        if enabled {
            tracing::warn!(
                target: "tui::perf::mem",
                "tui.performance.heap_profile_enabled is set but this build has no jemalloc \
                 control; launch through `just coco-jemalloc`"
            );
        }
        return;
    }
    tokio::task::spawn_blocking(move || {
        if !coco_utils_jemalloc::heap_profiling_available() {
            if enabled {
                tracing::warn!(
                    target: "tui::perf::mem",
                    "heap profiling requested but jemalloc started without `prof:true`; rebuild \
                     through `just coco-jemalloc` (the workspace bakes it in via \
                     JEMALLOC_SYS_WITH_MALLOC_CONF)"
                );
            }
            return;
        }
        match coco_utils_jemalloc::set_heap_profiling_active(enabled) {
            Ok(()) => {
                tracing::info!(
                    target: "tui::perf::mem",
                    enabled,
                    "jemalloc heap-profile sampling toggled"
                );
            }
            Err(err) => {
                tracing::warn!(
                    target: "tui::perf::mem",
                    %err,
                    enabled,
                    "jemalloc prof.active toggle failed"
                );
            }
        }
    });
}

/// Write one `prof.dump` next to the process logs:
/// `<config_home>/logs/coco.<pid>.turn<N>.heap`. Runs on the purge's blocking
/// task; silently skipped when profiling isn't available in this process.
fn dump_heap_profile(reason: &'static str) {
    if !coco_utils_jemalloc::heap_profiling_available() {
        return;
    }
    let dir = coco_config::global_config::config_home().join("logs");
    if let Err(err) = std::fs::create_dir_all(&dir) {
        tracing::warn!(
            target: "tui::perf::mem",
            %err,
            dir = %dir.display(),
            "heap profile directory creation failed"
        );
        return;
    }
    let seq = HEAP_DUMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("coco.{}.turn{seq}.heap", std::process::id()));
    match coco_utils_jemalloc::dump_heap_profile(&path) {
        Ok(()) => {
            tracing::info!(
                target: "tui::perf::mem",
                path = %path.display(),
                dump_seq = seq,
                reason,
                "jemalloc heap profile dumped"
            );
        }
        Err(err) => {
            tracing::warn!(
                target: "tui::perf::mem",
                %err,
                path = %path.display(),
                "jemalloc heap profile dump failed"
            );
        }
    }
}

fn log_purge(reason: &'static str, before: Option<JemallocStats>, after: Option<JemallocStats>) {
    let (Some(before), Some(after)) = (before, after) else {
        tracing::debug!(
            target: "tui::perf::mem",
            reason,
            "jemalloc arena purge; stats unavailable"
        );
        return;
    };
    tracing::debug!(
        target: "tui::perf::mem",
        reason,
        resident_before_bytes = before.resident,
        resident_after_bytes = after.resident,
        resident_reclaimed_bytes = before.resident.saturating_sub(after.resident),
        retained_before_bytes = before.retained,
        retained_after_bytes = after.retained,
        allocated_bytes = after.allocated,
        active_bytes = after.active,
        "jemalloc arena purge"
    );
}
