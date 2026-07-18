//! Always-on, bounded memory diagnostics for ordinary TUI runs.
//!
//! Unlike `MemoryPerfTracker`'s opt-in debug logs, this artifact survives the
//! default tracing filter. Small typed JSONL records go under
//! `<config_home>/logs/memtrace/`; the sink rotates by size, samples at a low
//! fixed cadence, and captures a bounded `malloc_stats_print` report only when
//! the process crosses a doubling memory bucket.

use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::SystemTime;

use crate::perf::MemoryObservation;
use crate::perf::MemoryPhase;
use crate::perf::MemorySampleKind;
use crate::perf::RetainedMemoryStats;
use coco_utils_jemalloc::JemallocStats;

mod sink;
use sink::JemallocTrace;
use sink::MemoryTraceEvent;
use sink::MemoryTraceInner;
use sink::ProcessTrace;
use sink::RetainedTrace;
#[cfg(test)]
use sink::backup_path;
use sink::now_ms;
use sink::open_append;
use sink::prune_directory;
use sink::truncate_utf8;

pub(crate) const SAMPLE_INTERVAL: Duration = Duration::from_secs(30);
const INITIAL_THRESHOLD_BYTES: u64 = 512 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const ROTATED_FILES: usize = 2;
const MAX_STATS_PRINT_BYTES: usize = 512 * 1024;
const MAX_DIRECTORY_FILES: usize = 24;
const MIN_CAP_DELETE_AGE: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Clone, Default)]
pub(crate) struct MemoryTrace {
    inner: Option<Arc<Mutex<MemoryTraceInner>>>,
    sequencer: Option<Arc<TraceSequencer>>,
}

impl MemoryTrace {
    pub(crate) fn open_default() -> Self {
        let dir = coco_config::global_config::config_home()
            .join("logs")
            .join("memtrace");
        match Self::open_at(&dir, std::process::id(), MAX_FILE_BYTES) {
            Ok(trace) => trace,
            Err(err) => {
                tracing::warn!(
                    target: "tui::perf::mem",
                    %err,
                    dir = %dir.display(),
                    "memory trace artifact unavailable"
                );
                Self::default()
            }
        }
    }

    fn open_at(dir: &Path, pid: u32, max_file_bytes: u64) -> io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        prune_directory(
            dir,
            pid,
            MAX_DIRECTORY_FILES,
            MIN_CAP_DELETE_AGE,
            SystemTime::now(),
        )?;
        let path = dir.join(format!("coco.{pid}.jsonl"));
        let writer = open_append(&path)?;
        let bytes_written = writer.get_ref().metadata()?.len();
        let mut inner = MemoryTraceInner {
            path,
            writer: Some(writer),
            bytes_written,
            max_file_bytes,
            rotated_files: ROTATED_FILES,
            thresholds: Thresholds::new(INITIAL_THRESHOLD_BYTES),
            write_error_reported: false,
        };
        if bytes_written >= max_file_bytes {
            inner.rotate()?;
        }
        Ok(Self {
            inner: Some(Arc::new(Mutex::new(inner))),
            sequencer: Some(Arc::new(TraceSequencer::default())),
        })
    }

    #[cfg(test)]
    pub(crate) fn open_for_test(dir: &Path, pid: u32, max_file_bytes: u64) -> io::Result<Self> {
        Self::open_at(dir, pid, max_file_bytes)
    }

    /// Reserve this sample's place while still on the UI thread. Blocking
    /// workers may start in any order; the ticket preserves lifecycle order
    /// without moving sampling or file I/O back onto the event loop.
    pub(crate) fn sample_job(
        &self,
        phase: MemoryPhase,
        sample_kind: MemorySampleKind,
    ) -> Option<MemoryTraceSampleJob> {
        let sequencer = self.sequencer.as_ref()?;
        let ticket = sequencer.next_ticket.fetch_add(1, Ordering::Relaxed);
        Some(MemoryTraceSampleJob {
            trace: self.clone(),
            sequencer: Arc::clone(sequencer),
            ticket,
            phase,
            sample_kind,
        })
    }

    /// Reserve a purge immediately after its lifecycle sample. The purge
    /// itself waits on a blocking worker, so it cannot mutate allocator state
    /// before the corresponding pre-purge sample or overtake that sample in
    /// the JSONL event stream.
    pub(crate) fn purge_job(&self) -> MemoryTracePurgeJob {
        let reservation = self.sequencer.as_ref().map(|sequencer| {
            let ticket = sequencer.next_ticket.fetch_add(1, Ordering::Relaxed);
            (Arc::clone(sequencer), ticket)
        });
        MemoryTracePurgeJob {
            trace: self.clone(),
            reservation,
        }
    }

    /// Persist a sample and return a threshold crossing, if this observation
    /// should trigger the expensive allocator report on a blocking thread.
    pub(crate) fn record_sample(
        &self,
        phase: MemoryPhase,
        sample_kind: MemorySampleKind,
        observation: MemoryObservation,
    ) -> Option<ThresholdCrossing> {
        self.record_sample_inner(phase, sample_kind, observation, true)
    }

    fn record_sample_inner(
        &self,
        phase: MemoryPhase,
        sample_kind: MemorySampleKind,
        observation: MemoryObservation,
        retained_available: bool,
    ) -> Option<ThresholdCrossing> {
        let inner = self.inner.as_ref()?;
        let mut inner = inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let gauge_bytes = observation
            .process
            .map(|sample| sample.physical_footprint_bytes.unwrap_or(sample.rss_bytes))
            .or_else(|| observation.jemalloc.map(|stats| stats.resident));
        let crossing = gauge_bytes.and_then(|bytes| inner.thresholds.observe(bytes));
        let event = MemoryTraceEvent::Sample {
            timestamp_ms: now_ms(),
            phase,
            sample_kind,
            process: observation.process.map(ProcessTrace::from),
            jemalloc: observation.jemalloc.map(JemallocTrace::from),
            retained: retained_available.then(|| RetainedTrace::from(observation.retained)),
            gauge_bytes,
            next_threshold_bytes: inner.thresholds.next_bytes,
        };
        inner.write(&event);
        crossing.map(|crossing| ThresholdCrossing {
            phase,
            gauge_bytes: crossing.gauge_bytes,
            crossed_threshold_bytes: crossing.crossed_threshold_bytes,
            next_threshold_bytes: crossing.next_threshold_bytes,
        })
    }

    /// Capture the large allocator report only after a crossing, off the event
    /// loop. The serialized text is capped so one record cannot defeat file
    /// rotation or consume unbounded disk.
    pub(crate) fn record_threshold_dump(&self, crossing: ThresholdCrossing) {
        let stats = coco_utils_jemalloc::stats_print();
        let (stats_print, stats_print_truncated) = stats.as_deref().map_or((None, false), |text| {
            let (text, truncated) = truncate_utf8(text, MAX_STATS_PRINT_BYTES);
            (Some(text), truncated)
        });
        let event = MemoryTraceEvent::ThresholdCrossing {
            timestamp_ms: now_ms(),
            phase: crossing.phase,
            gauge_bytes: crossing.gauge_bytes,
            crossed_threshold_bytes: crossing.crossed_threshold_bytes,
            next_threshold_bytes: crossing.next_threshold_bytes,
            stats_print,
            stats_print_truncated,
        };
        self.write(&event);
    }

    /// Blocking worker entry: process/allocator sampling, JSON serialization,
    /// disk flush, and the optional large stats dump all stay off the UI loop.
    pub(crate) fn capture_and_record(&self, phase: MemoryPhase, sample_kind: MemorySampleKind) {
        let observation = crate::perf::capture_memory_observation(RetainedMemoryStats::default());
        if let Some(crossing) = self.record_sample_inner(phase, sample_kind, observation, false) {
            self.record_threshold_dump(crossing);
        }
    }

    pub(crate) fn record_observation(
        &self,
        phase: MemoryPhase,
        sample_kind: MemorySampleKind,
        observation: MemoryObservation,
    ) {
        if let Some(crossing) = self.record_sample(phase, sample_kind, observation) {
            self.record_threshold_dump(crossing);
        }
    }

    pub(crate) fn record_purge(
        &self,
        phase: MemoryPhase,
        before: Option<JemallocStats>,
        after: Option<JemallocStats>,
        duration: Duration,
        error: Option<&str>,
    ) {
        let event = MemoryTraceEvent::Purge {
            timestamp_ms: now_ms(),
            phase,
            before: before.map(JemallocTrace::from),
            after: after.map(JemallocTrace::from),
            resident_reclaimed_bytes: before
                .zip(after)
                .map(|(before, after)| before.resident.saturating_sub(after.resident)),
            duration_ms: duration.as_millis(),
            error,
        };
        self.write(&event);
    }

    fn write(&self, event: &MemoryTraceEvent<'_>) {
        let Some(inner) = &self.inner else {
            return;
        };
        inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .write(event);
    }
}

pub(crate) struct MemoryTraceSampleJob {
    trace: MemoryTrace,
    sequencer: Arc<TraceSequencer>,
    ticket: u64,
    phase: MemoryPhase,
    sample_kind: MemorySampleKind,
}

impl MemoryTraceSampleJob {
    pub(crate) fn run(self, observation: Option<MemoryObservation>) {
        let _turn = self.sequencer.enter(self.ticket);
        match observation {
            Some(observation) => {
                self.trace
                    .record_observation(self.phase, self.sample_kind, observation);
            }
            None => self.trace.capture_and_record(self.phase, self.sample_kind),
        }
    }
}

pub(crate) struct MemoryTracePurgeJob {
    trace: MemoryTrace,
    reservation: Option<(Arc<TraceSequencer>, u64)>,
}

impl MemoryTracePurgeJob {
    pub(crate) fn run(self, purge: impl FnOnce(&MemoryTrace)) {
        let _turn = self
            .reservation
            .as_ref()
            .map(|(sequencer, ticket)| sequencer.enter(*ticket));
        purge(&self.trace);
    }

    #[cfg(test)]
    fn is_ready_for_test(&self) -> bool {
        self.reservation.as_ref().is_none_or(|(sequencer, ticket)| {
            *sequencer
                .current_ticket
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                == *ticket
        })
    }
}

#[derive(Debug, Default)]
struct TraceSequencer {
    next_ticket: AtomicU64,
    current_ticket: Mutex<u64>,
    ready: Condvar,
}

impl TraceSequencer {
    fn enter(&self, ticket: u64) -> TraceTurn<'_> {
        let mut current = self
            .current_ticket
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while *current != ticket {
            current = self
                .ready
                .wait(current)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        TraceTurn { sequencer: self }
    }
}

struct TraceTurn<'a> {
    sequencer: &'a TraceSequencer,
}

impl Drop for TraceTurn<'_> {
    fn drop(&mut self) {
        let mut current = self
            .sequencer
            .current_ticket
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *current = current.saturating_add(1);
        drop(current);
        self.sequencer.ready.notify_all();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ThresholdCrossing {
    phase: MemoryPhase,
    gauge_bytes: u64,
    crossed_threshold_bytes: u64,
    next_threshold_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Crossing {
    gauge_bytes: u64,
    crossed_threshold_bytes: u64,
    next_threshold_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Thresholds {
    floor_bytes: u64,
    next_bytes: u64,
}

impl Thresholds {
    fn new(floor_bytes: u64) -> Self {
        let floor_bytes = floor_bytes.max(1);
        Self {
            floor_bytes,
            next_bytes: floor_bytes,
        }
    }

    fn observe(&mut self, gauge_bytes: u64) -> Option<Crossing> {
        while self.next_bytes > self.floor_bytes && gauge_bytes < self.next_bytes / 2 {
            self.next_bytes = (self.next_bytes / 2).max(self.floor_bytes);
        }
        if gauge_bytes < self.next_bytes {
            return None;
        }

        let crossed_threshold_bytes = self.next_bytes;
        while gauge_bytes >= self.next_bytes {
            let doubled = self.next_bytes.saturating_mul(2);
            if doubled == self.next_bytes {
                break;
            }
            self.next_bytes = doubled;
        }
        Some(Crossing {
            gauge_bytes,
            crossed_threshold_bytes,
            next_threshold_bytes: self.next_bytes,
        })
    }
}

#[cfg(test)]
#[path = "memory_trace.test.rs"]
mod tests;
