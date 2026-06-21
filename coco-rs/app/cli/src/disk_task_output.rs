//! Disk-backed task output buffers.
//!
//! ## Why
//!
//! Background AgentTool spawns can run arbitrarily long. The
//! original in-memory buffer (8 MiB cap, head-truncate) is fine for
//! short spawns but loses early context on long-running coordinator
//! workloads. This module appends output to disk asynchronously and
//! reads return incremental ranges via `pread`.
//!
//! ## Semantics
//!
//! | Concept | coco-rs |
//! |---|---|
//! | Session output dir | `coco_config::config_home().join("cache/tasks").join(session_id)` |
//! | Per-task file | `{taskId}.output` |
//! | Open flags (Unix) | `OpenOptions().create(true).append(true)` + `custom_flags(O_NOFOLLOW)` |
//! | Disk cap | `MAX_TASK_OUTPUT_BYTES = 5 GB`; `[output truncated: exceeded 5GB disk cap]` marker |
//! | Write queue | `tokio::sync::mpsc::UnboundedSender` + single drain task |
//! | flush | `flush()` returns `oneshot::Receiver<()>` resolved when the drain catches up |
//! | cancel | `cancel()` aborts the drain task |
//! | incremental read | `read_delta(from_offset, max_bytes)` via `tokio::fs::File::read_at` |
//! | Per-session registry | `DiskOutputs::new(session_dir)` captured at construction |
//! | pending-ops counter | `pending_ops: Arc<AtomicI64>` counter + `wait_quiescent` |
//!
//! ## Intentional differences
//!
//! - **No symlink mode.** The `coco_session::TranscriptStore` uses a
//!   sharded JSONL layout that can't be exposed as a single tail-able
//!   file. Future work when the transcript layout stabilises.
//! - **No Windows string-mode fallback.** Production runs on Linux;
//!   OpenOptions is portable.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use tokio::fs::{File, OpenOptions, create_dir_all, remove_file};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom};
use tokio::sync::{Mutex, RwLock, mpsc, oneshot};

/// 5 GB disk cap. Past this, the writer drops chunks and appends a
/// single truncation marker.
pub const MAX_TASK_OUTPUT_BYTES: i64 = 5 * 1024 * 1024 * 1024;

/// Default per-call delta read cap (8 MiB).
pub const DEFAULT_MAX_READ_BYTES: usize = 8 * 1024 * 1024;

/// Truncation marker appended once when a task crosses the disk cap.
const TRUNCATION_MARKER: &str = "\n[output truncated: exceeded 5GB disk cap]\n";

/// Operations the drain loop processes.
enum DiskOp {
    Append(String),
    Flush(oneshot::Sender<()>),
    Shutdown,
}

/// Async-disk output handle for a single task. Cloning yields
/// independent `Arc` references; the underlying drain task is
/// shared. Cheap to clone.
#[derive(Clone)]
pub struct DiskTaskOutput {
    inner: Arc<DiskTaskOutputInner>,
}

struct DiskTaskOutputInner {
    path: PathBuf,
    tx: mpsc::UnboundedSender<DiskOp>,
    /// Bytes appended (pre-truncation accounting). Read for the cap
    /// check inside `append`. Atomic so callers can `flush_size_hint`
    /// without taking a lock.
    bytes_written: AtomicI64,
    /// One-shot latch flipped when the cap is hit so the truncation
    /// marker appends exactly once.
    capped: AtomicBool,
    /// Counter of in-flight async ops (`Append` queue depth +
    /// pending flushes). Incremented on send, decremented when the
    /// drain processes the op. Used by `wait_quiescent` for tests
    /// + clean shutdown.
    pending_ops: Arc<AtomicI64>,
}

impl DiskTaskOutput {
    /// Construct and spawn the drain task. The file is created lazily
    /// on first append.
    pub fn new(path: PathBuf) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let pending_ops = Arc::new(AtomicI64::new(0));
        let inner = Arc::new(DiskTaskOutputInner {
            path: path.clone(),
            tx,
            bytes_written: AtomicI64::new(0),
            capped: AtomicBool::new(false),
            pending_ops: pending_ops.clone(),
        });
        let drain_path = path;
        let drain_pending = pending_ops;
        tokio::spawn(async move { drain_loop(drain_path, rx, drain_pending).await });
        Self { inner }
    }

    /// Path to the on-disk output file. Useful for `TaskOutput` /
    /// callers that want the path itself.
    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    /// Append a chunk. Returns immediately — the actual fs write
    /// runs on the drain task. Past the 5 GB cap, drops the chunk
    /// and (once) enqueues the truncation marker.
    pub fn append(&self, chunk: &str) {
        if self.inner.capped.load(Ordering::Acquire) {
            return;
        }
        // Count in bytes (not UTF-16 code units) for the cap check.
        let added = chunk.len() as i64;
        let total = self.inner.bytes_written.fetch_add(added, Ordering::AcqRel) + added;
        if total > MAX_TASK_OUTPUT_BYTES {
            // First over-cap caller wins the marker; concurrent
            // others just drop silently.
            if !self.inner.capped.swap(true, Ordering::AcqRel) {
                self.inner.pending_ops.fetch_add(1, Ordering::AcqRel);
                let _ = self
                    .inner
                    .tx
                    .send(DiskOp::Append(TRUNCATION_MARKER.to_string()));
            }
            return;
        }
        self.inner.pending_ops.fetch_add(1, Ordering::AcqRel);
        let _ = self.inner.tx.send(DiskOp::Append(chunk.to_string()));
    }

    /// Wait for the drain task to process every queued chunk up to
    /// this call. Used by `read_delta` callers that want a freshly-
    /// flushed view.
    pub async fn flush(&self) -> Result<(), &'static str> {
        let (tx, rx) = oneshot::channel();
        self.inner.pending_ops.fetch_add(1, Ordering::AcqRel);
        if self.inner.tx.send(DiskOp::Flush(tx)).is_err() {
            return Err("drain task closed");
        }
        rx.await.map_err(|_| "flush sender dropped")
    }

    /// Read `[from_offset, from_offset + max_bytes)` of the output
    /// file. Returns the read content + new offset.
    pub async fn read_delta(
        &self,
        from_offset: i64,
        max_bytes: usize,
    ) -> std::io::Result<(String, i64)> {
        let from = from_offset.max(0) as u64;
        let mut file = match File::open(&self.inner.path).await {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok((String::new(), from_offset));
            }
            Err(e) => return Err(e),
        };
        file.seek(SeekFrom::Start(from)).await?;
        let mut buf = vec![0u8; max_bytes];
        let n = file.read(&mut buf).await?;
        buf.truncate(n);
        // Lossy because the buffer might cut a UTF-8 codepoint.
        let content = String::from_utf8_lossy(&buf).into_owned();
        Ok((content, from_offset + n as i64))
    }

    /// Total size of the output file in bytes.
    pub async fn size(&self) -> i64 {
        match tokio::fs::metadata(&self.inner.path).await {
            Ok(m) => m.len() as i64,
            Err(_) => 0,
        }
    }

    /// Read the **tail** of the output file (last `max_bytes`
    /// bytes), prepending an "[N KB earlier output omitted]\n"
    /// header when the file exceeded `max_bytes`.
    ///
    /// This is the right shape for the periodic AgentSummary timer
    /// and for `TaskOutput` model-facing reads — model sees the
    /// recent activity, not the cold start. Use [`Self::read_delta`]
    /// when an offset-based incremental reader is needed.
    pub async fn read_tail(&self, max_bytes: usize) -> std::io::Result<String> {
        let total = self.size().await;
        let total_u = total.max(0) as u64;
        if total_u == 0 {
            return Ok(String::new());
        }
        let from = total_u.saturating_sub(max_bytes as u64);
        let mut file = match File::open(&self.inner.path).await {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
            Err(e) => return Err(e),
        };
        file.seek(SeekFrom::Start(from)).await?;
        let mut buf = Vec::with_capacity((total_u - from) as usize);
        let _ = file.read_to_end(&mut buf).await?;
        let content = String::from_utf8_lossy(&buf).into_owned();
        let omitted = total_u.saturating_sub(buf.len() as u64);
        if omitted > 0 {
            // Round to KB for the omitted-bytes header.
            let kb = ((omitted as f64) / 1024.0).round() as u64;
            Ok(format!("[{kb}KB of earlier output omitted]\n{content}"))
        } else {
            Ok(content)
        }
    }

    /// Cancel the drain task. Pending writes are dropped. The file
    /// itself is left intact — call `cleanup` to unlink.
    pub fn cancel(&self) {
        let _ = self.inner.tx.send(DiskOp::Shutdown);
    }

    /// Unlink the output file. Best-effort; missing file is OK.
    pub async fn cleanup(&self) -> std::io::Result<()> {
        match remove_file(&self.inner.path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Block until the drain queue is empty (for tests and clean
    /// shutdown). Polls the pending-ops counter; not exposed for
    /// production — production callers should use `flush`.
    pub async fn wait_quiescent(&self) {
        while self.inner.pending_ops.load(Ordering::Acquire) > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}

async fn drain_loop(
    path: PathBuf,
    mut rx: mpsc::UnboundedReceiver<DiskOp>,
    pending_ops: Arc<AtomicI64>,
) {
    // Lazily open on first append. Closing on shutdown lets the
    // OS reclaim the fd; reopening on the next append is fine.
    let mut file: Option<File> = None;
    while let Some(op) = rx.recv().await {
        match op {
            DiskOp::Append(chunk) => {
                if let Err(e) = ensure_open(&path, &mut file).await {
                    tracing::debug!(?path, error = %e, "DiskTaskOutput: open failed; dropping chunk");
                } else if let Some(fh) = file.as_mut()
                    && let Err(e) = fh.write_all(chunk.as_bytes()).await
                {
                    tracing::debug!(?path, error = %e, "DiskTaskOutput: write failed; dropping chunk");
                }
                pending_ops.fetch_sub(1, Ordering::AcqRel);
            }
            DiskOp::Flush(reply) => {
                if let Some(fh) = file.as_mut() {
                    let _ = fh.flush().await;
                }
                let _ = reply.send(());
                pending_ops.fetch_sub(1, Ordering::AcqRel);
            }
            DiskOp::Shutdown => {
                if let Some(mut fh) = file.take() {
                    let _ = fh.flush().await;
                    let _ = fh.shutdown().await;
                }
                break;
            }
        }
    }
}

async fn ensure_open(path: &Path, slot: &mut Option<File>) -> std::io::Result<()> {
    if slot.is_some() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        create_dir_all(parent).await?;
    }
    let mut opts = OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        // O_NOFOLLOW: prevents a sandbox process from creating a
        // symlink at the output path pointing at an arbitrary file
        // we'd then write to. `tokio::fs::OpenOptions::custom_flags`
        // is inherent on `cfg(unix)` so no trait import needed.
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    let file = opts.open(path).await?;
    *slot = Some(file);
    Ok(())
}

/// Per-session registry of disk task outputs. Explicit and per-session
/// so `/clear` regen doesn't conflate sessions.
pub struct DiskOutputs {
    /// Resolved on construction. Captured early so subsequent `/clear`
    /// session regen doesn't invalidate paths held by in-flight
    /// `DiskTaskOutput` instances.
    session_dir: PathBuf,
    outputs: RwLock<HashMap<String, DiskTaskOutput>>,
    init_lock: Mutex<()>,
}

impl DiskOutputs {
    /// `session_dir` is typically
    /// `coco_config::config_home().join("cache/tasks").join(session_id)`.
    pub fn new(session_dir: PathBuf) -> Self {
        Self {
            session_dir,
            outputs: RwLock::new(HashMap::new()),
            init_lock: Mutex::new(()),
        }
    }

    pub fn output_path(&self, task_id: &str) -> PathBuf {
        self.session_dir.join(format!("{task_id}.output"))
    }

    /// Get-or-create an entry for `task_id`. The init lock prevents
    /// two concurrent `get_or_create` calls from spawning two drain
    /// loops for the same id.
    pub async fn get_or_create(&self, task_id: &str) -> DiskTaskOutput {
        if let Some(existing) = self.outputs.read().await.get(task_id) {
            return existing.clone();
        }
        let _g = self.init_lock.lock().await;
        if let Some(existing) = self.outputs.read().await.get(task_id) {
            return existing.clone();
        }
        let path = self.output_path(task_id);
        let dto = DiskTaskOutput::new(path);
        self.outputs
            .write()
            .await
            .insert(task_id.to_string(), dto.clone());
        dto
    }

    /// Look up an entry without creating one.
    pub async fn get(&self, task_id: &str) -> Option<DiskTaskOutput> {
        self.outputs.read().await.get(task_id).cloned()
    }

    /// Evict the in-memory handle (cancels the drain) but leaves
    /// the file on disk. Use when a task completes and the output
    /// is no longer needed.
    pub async fn evict(&self, task_id: &str) {
        let dto = self.outputs.write().await.remove(task_id);
        if let Some(dto) = dto {
            // Best-effort flush before dropping the drain.
            let _ = dto.flush().await;
            dto.cancel();
        }
    }

    /// Evict + unlink the file.
    pub async fn cleanup(&self, task_id: &str) -> std::io::Result<()> {
        let dto = self.outputs.write().await.remove(task_id);
        if let Some(dto) = dto {
            dto.cancel();
            return dto.cleanup().await;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "disk_task_output.test.rs"]
mod tests;
