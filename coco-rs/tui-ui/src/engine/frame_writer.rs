//! Non-blocking terminal frame delivery.
//!
//! Ratatui emits incremental terminal diffs, so queued production frames are
//! lossless: while one frame is being written, subsequent frames coalesce in
//! the single pending slot. The generic writer also supports replacing an
//! explicitly self-contained pending frame, but callers must never apply that
//! policy to an incremental diff or native-history append.

use std::io;
use std::io::BufWriter;
use std::io::Write;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;
use std::time::Instant;

use super::terminal::TerminalWriteStats;

const WRITER_BUFFER_BYTES: usize = 64 * 1024;
const MAX_PENDING_BYTES: usize = 8 * 1024 * 1024;
const DROP_DRAIN_TIMEOUT: Duration = Duration::from_millis(250);

/// Whether a pending frame may supersede an older pending frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameDelivery {
    /// A delta against the previously submitted terminal state. It must be
    /// delivered in order; pending deltas are coalesced into one write batch.
    Incremental,
    /// A complete terminal image that does not depend on an older pending
    /// frame. Only this variant may replace another pending batch.
    SelfContained,
    /// The terminal restore/prompt tail emitted during application teardown.
    /// It is appended after all earlier output and may bypass the ordinary
    /// backlog limit so a delayed writer always finishes in a safe state.
    Teardown,
}

/// Construction options supplied by the application shell.
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameWriterOptions {
    /// Test-only latency injection. The UI crate deliberately does not read
    /// environment variables; the shell resolves configuration and passes it.
    pub write_delay: Duration,
}

/// Monotonic writer counters useful for diagnostics and tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameWriterCounters {
    pub queued: u64,
    pub written: u64,
    pub dropped: u64,
}

/// A cloneable barrier for tty handoffs and teardown.
#[derive(Clone)]
pub struct DrainBarrier {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for DrainBarrier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DrainBarrier")
            .field("counters", &self.counters())
            .finish_non_exhaustive()
    }
}

impl DrainBarrier {
    /// Wait until every frame queued before this call has either been written
    /// or superseded by a self-contained frame.
    pub fn wait_drained(&self, timeout: Duration) -> io::Result<bool> {
        let target = self.shared.queued.load(Ordering::Acquire);
        let deadline = Instant::now() + timeout;
        let mut state = self.shared.lock_state()?;
        loop {
            if let Some(error) = &state.error {
                return Err(error.to_io_error());
            }
            if state.completed_through >= target {
                return Ok(true);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(false);
            }
            let (next, timed_out) = self
                .shared
                .drained
                .wait_timeout(state, remaining)
                .map_err(|_| io::Error::other("terminal writer drain mutex poisoned"))?;
            state = next;
            if timed_out.timed_out() && state.completed_through < target {
                return Ok(false);
            }
        }
    }

    pub fn counters(&self) -> FrameWriterCounters {
        FrameWriterCounters {
            queued: self.shared.queued.load(Ordering::Acquire),
            written: self.shared.written.load(Ordering::Acquire),
            dropped: self.shared.dropped.load(Ordering::Acquire),
        }
    }

    pub fn latest_write_stats(&self) -> Option<TerminalWriteStats> {
        let through_sequence = self.shared.latest_written_through.load(Ordering::Acquire);
        (through_sequence > 0).then(|| TerminalWriteStats {
            through_sequence,
            elapsed: Duration::from_micros(self.shared.latest_write_micros.load(Ordering::Relaxed)),
        })
    }

    #[cfg(test)]
    fn wait_until_in_flight(&self, timeout: Duration) -> io::Result<bool> {
        let deadline = Instant::now() + timeout;
        let mut state = self.shared.lock_state()?;
        while !state.in_flight {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(false);
            }
            let (next, timed_out) = self
                .shared
                .drained
                .wait_timeout(state, remaining)
                .map_err(|_| io::Error::other("terminal writer drain mutex poisoned"))?;
            state = next;
            if timed_out.timed_out() && !state.in_flight {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// `io::Write` sink that accumulates one terminal frame and hands completed
/// frames to a named OS thread.
pub struct FrameWriter<W>
where
    W: Write + Send + 'static,
{
    frame: Vec<u8>,
    barrier: DrainBarrier,
    worker: Option<JoinHandle<()>>,
    _writer: std::marker::PhantomData<W>,
}

impl<W> std::fmt::Debug for FrameWriter<W>
where
    W: Write + Send + 'static,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameWriter")
            .field("buffered_bytes", &self.frame.len())
            .field("barrier", &self.barrier)
            .finish_non_exhaustive()
    }
}

impl<W> FrameWriter<W>
where
    W: Write + Send + 'static,
{
    pub fn new(writer: W, options: FrameWriterOptions) -> io::Result<Self> {
        let shared = Arc::new(Shared::default());
        let worker_shared = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("coco-term-writer".to_owned())
            .spawn(move || writer_loop(writer, worker_shared, options.write_delay))?;
        Ok(Self {
            frame: Vec::with_capacity(WRITER_BUFFER_BYTES),
            barrier: DrainBarrier { shared },
            worker: Some(worker),
            _writer: std::marker::PhantomData,
        })
    }

    pub fn barrier(&self) -> DrainBarrier {
        self.barrier.clone()
    }

    /// Queue the bytes accumulated since the previous presentation.
    pub fn present(&mut self, delivery: FrameDelivery) -> io::Result<()> {
        if self.frame.is_empty() {
            return self.barrier.shared.current_error();
        }

        let mut state = self.barrier.shared.lock_state()?;
        if let Some(error) = &state.error {
            return Err(error.to_io_error());
        }
        let pending_bytes = state
            .pending
            .as_ref()
            .map_or(0, |pending| pending.bytes.len());
        let projected_bytes = if delivery == FrameDelivery::SelfContained {
            self.frame.len()
        } else {
            pending_bytes.saturating_add(self.frame.len())
        };
        if delivery != FrameDelivery::Teardown && projected_bytes > MAX_PENDING_BYTES {
            // A rejected incremental frame must have a terminal state. Keeping
            // it in the local buffer would let a later teardown delivery
            // smuggle it past the cap and would retain an arbitrarily large
            // allocation for subsequent writes.
            self.frame = Vec::with_capacity(WRITER_BUFFER_BYTES);
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "terminal frame backlog exceeded 8 MiB",
            ));
        }

        let next_capacity = WRITER_BUFFER_BYTES.min(self.frame.capacity().max(1));
        let bytes = std::mem::replace(&mut self.frame, Vec::with_capacity(next_capacity));
        let sequence = self.barrier.shared.queued.fetch_add(1, Ordering::AcqRel) + 1;

        let next = PendingFrame {
            bytes,
            through_sequence: sequence,
            frame_count: 1,
            delivery,
        };
        match state.pending.as_mut() {
            None => state.pending = Some(next),
            Some(pending) if delivery == FrameDelivery::SelfContained => {
                self.barrier
                    .shared
                    .dropped
                    .fetch_add(pending.frame_count, Ordering::AcqRel);
                *pending = next;
            }
            Some(pending) => {
                pending.bytes.extend_from_slice(&next.bytes);
                pending.through_sequence = next.through_sequence;
                pending.frame_count = pending.frame_count.saturating_add(1);
                pending.delivery = if delivery == FrameDelivery::Teardown {
                    FrameDelivery::Teardown
                } else {
                    FrameDelivery::Incremental
                };
            }
        }
        drop(state);
        self.barrier.shared.ready.notify_one();
        Ok(())
    }
}

impl<W> Write for FrameWriter<W>
where
    W: Write + Send + 'static,
{
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.barrier.shared.current_error()?;
        self.frame.extend_from_slice(buf);
        Ok(buf.len())
    }

    /// Inner ratatui flushes delimit logical sub-stages, not complete frames.
    /// Presentation is explicit at ESU so no partial escape transaction can
    /// reach the writer thread.
    fn flush(&mut self) -> io::Result<()> {
        self.barrier.shared.current_error()
    }
}

impl<W> Drop for FrameWriter<W>
where
    W: Write + Send + 'static,
{
    fn drop(&mut self) {
        let _ = self.present(FrameDelivery::Incremental);
        if let Ok(mut state) = self.barrier.shared.state.lock() {
            state.shutdown = true;
        }
        self.barrier.shared.ready.notify_one();

        // Normal application teardown drains explicitly before the backend is
        // dropped. Keep Drop bounded for error/panic paths where the OS writer
        // itself may be wedged.
        if self
            .barrier
            .wait_drained(DROP_DRAIN_TIMEOUT)
            .unwrap_or(false)
            && let Some(worker) = self.worker.take()
        {
            let _ = worker.join();
        }
    }
}

#[derive(Default)]
struct Shared {
    state: Mutex<WriterState>,
    ready: Condvar,
    drained: Condvar,
    queued: AtomicU64,
    written: AtomicU64,
    dropped: AtomicU64,
    latest_written_through: AtomicU64,
    latest_write_micros: AtomicU64,
}

impl Shared {
    fn lock_state(&self) -> io::Result<std::sync::MutexGuard<'_, WriterState>> {
        self.state
            .lock()
            .map_err(|_| io::Error::other("terminal writer mutex poisoned"))
    }

    fn current_error(&self) -> io::Result<()> {
        let state = self.lock_state()?;
        match &state.error {
            Some(error) => Err(error.to_io_error()),
            None => Ok(()),
        }
    }
}

#[derive(Default)]
struct WriterState {
    pending: Option<PendingFrame>,
    in_flight: bool,
    completed_through: u64,
    shutdown: bool,
    error: Option<WriterError>,
}

struct PendingFrame {
    bytes: Vec<u8>,
    through_sequence: u64,
    frame_count: u64,
    delivery: FrameDelivery,
}

#[derive(Clone)]
struct WriterError {
    kind: io::ErrorKind,
    message: Arc<str>,
}

impl WriterError {
    fn from_io_error(error: io::Error) -> Self {
        Self {
            kind: error.kind(),
            message: Arc::from(error.to_string()),
        }
    }

    fn to_io_error(&self) -> io::Error {
        io::Error::new(self.kind, self.message.to_string())
    }
}

fn writer_loop<W>(writer: W, shared: Arc<Shared>, write_delay: Duration)
where
    W: Write,
{
    let mut writer = BufWriter::with_capacity(WRITER_BUFFER_BYTES, writer);
    loop {
        let pending = {
            let mut state = match shared.state.lock() {
                Ok(state) => state,
                Err(_) => return,
            };
            while state.pending.is_none() && !state.shutdown {
                state = match shared.ready.wait(state) {
                    Ok(state) => state,
                    Err(_) => return,
                };
            }
            let Some(pending) = state.pending.take() else {
                return;
            };
            state.in_flight = true;
            shared.drained.notify_all();
            pending
        };

        let started = Instant::now();
        if !write_delay.is_zero() {
            thread::sleep(write_delay);
        }
        let result = writer
            .write_all(&pending.bytes)
            .and_then(|()| writer.flush());
        let elapsed = started.elapsed();

        let mut state = match shared.state.lock() {
            Ok(state) => state,
            Err(_) => return,
        };
        state.in_flight = false;
        match result {
            Ok(()) => {
                state.completed_through = pending.through_sequence;
                shared
                    .written
                    .fetch_add(pending.frame_count, Ordering::AcqRel);
                shared.latest_write_micros.store(
                    elapsed.as_micros().min(u128::from(u64::MAX)) as u64,
                    Ordering::Relaxed,
                );
                // Publish the paired latency last; readers acquire this
                // sequence before reading `latest_write_micros`.
                shared
                    .latest_written_through
                    .store(pending.through_sequence, Ordering::Release);
            }
            Err(error) => {
                state.error = Some(WriterError::from_io_error(error));
                state.pending = None;
                state.shutdown = true;
            }
        }
        shared.drained.notify_all();
    }
}

#[cfg(test)]
#[path = "frame_writer.test.rs"]
mod tests;
