//! Append-only JSONL journal primitive — the write/read/rotate mechanism for
//! observability planes (the learning-timeline journal today).
//!
//! Policy-free by design: the concrete record type is bound by the caller via
//! the `Serialize` / `DeserializeOwned` generic, so this crate stays free of a
//! `coco-types` dependency (consistent with [`crate::lock`] / [`crate::write_fence`]).
//! The *record vocabulary* stays in the domain crate that owns it; only the
//! bounded-growth mechanism lives here.
//!
//! **Best-effort:** an observability plane must never disturb the main loop, so
//! every failure here is `tracing`-only and never propagated. The reader skips
//! any line it cannot parse (schema drift, a torn write) with a count, so one
//! bad line never poisons the whole view.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Single-generation rotation ceiling (4 MiB ≈ 15k low-frequency events).
///
/// Owned here rather than by each write site: it is one mechanism default for
/// every journal this crate serves, and a per-caller copy is exactly how four
/// of them drift apart.
pub const DEFAULT_MAX_BYTES: u64 = 4 * 1024 * 1024;

/// Rotate past the ceiling, then append — the only supported way to write a
/// journal that must not grow without bound.
///
/// Bundled deliberately: [`append_jsonl`] alone never rotates, so leaving the
/// pairing to call sites makes unbounded growth the *default* outcome of
/// forgetting one line. Callers get bounded growth by construction instead.
///
/// Blocking I/O — call from `spawn_blocking` in async contexts.
pub fn append_rotating<T: Serialize>(path: &Path, record: &T) {
    rotate_if_over(path, DEFAULT_MAX_BYTES);
    append_jsonl(path, record);
}

/// Append one JSON record as a single line to an append-only journal.
///
/// Best-effort: any failure is `tracing::warn` only, never propagated. POSIX
/// `O_APPEND` makes offset positioning atomic; small single `write()`s on local
/// filesystems do not interleave in practice (no hard spec guarantee — NFS can
/// break it). The reader's skip-corrupt-line behavior absorbs the residual risk.
///
/// Blocking I/O — call from `spawn_blocking` in async contexts.
pub fn append_jsonl<T: Serialize>(path: &Path, record: &T) {
    let mut line = match serde_json::to_string(record) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                target: "coco_maintenance::journal",
                path = %path.display(),
                "journal serialize failed: {e}"
            );
            return;
        }
    };
    // One record == one line: a serialized JSON value never contains a raw
    // newline, so a single trailing `\n` keeps the line framing intact.
    line.push('\n');
    let file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path);
    match file {
        Ok(mut f) => {
            if let Err(e) = f.write_all(line.as_bytes()) {
                tracing::warn!(
                    target: "coco_maintenance::journal",
                    path = %path.display(),
                    "journal append failed: {e}"
                );
            }
        }
        Err(e) => {
            tracing::warn!(
                target: "coco_maintenance::journal",
                path = %path.display(),
                "journal open failed: {e}"
            );
        }
    }
}

/// Read the whole journal back, skipping any line that fails to deserialize.
///
/// Corrupt / schema-drifted lines are skipped with a `tracing::debug` count.
/// A missing file yields an empty `Vec`.
///
/// Blocking I/O — call from `spawn_blocking` in async contexts.
pub fn read_jsonl<T: DeserializeOwned>(path: &Path) -> Vec<T> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            tracing::warn!(
                target: "coco_maintenance::journal",
                path = %path.display(),
                "journal open-for-read failed: {e}"
            );
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    let mut skipped = 0usize;
    for line in BufReader::new(file).lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<T>(&line) {
            Ok(record) => out.push(record),
            Err(_) => skipped += 1,
        }
    }
    if skipped > 0 {
        tracing::debug!(
            target: "coco_maintenance::journal",
            path = %path.display(),
            skipped,
            "skipped corrupt journal lines"
        );
    }
    out
}

/// Size guard: when the file exceeds `max_bytes`, rename it to `<path>.1`
/// (single generation; an existing `.1` is overwritten). Concurrent rotation
/// from two processes is benign: rename is atomic and the loser's failure is
/// ignored.
///
/// Crate-private on purpose — [`append_rotating`] is the only caller, so the
/// rotate-before-append ordering cannot be got wrong from outside.
///
/// Blocking I/O — call from `spawn_blocking` in async contexts.
pub(crate) fn rotate_if_over(path: &Path, max_bytes: u64) {
    let len = match std::fs::metadata(path) {
        Ok(m) => m.len(),
        Err(_) => return, // missing / unreadable → nothing to rotate
    };
    if len <= max_bytes {
        return;
    }
    let mut rotated = path.as_os_str().to_owned();
    rotated.push(".1");
    if let Err(e) = std::fs::rename(path, &rotated) {
        tracing::debug!(
            target: "coco_maintenance::journal",
            path = %path.display(),
            "journal rotation failed (benign): {e}"
        );
    }
}

#[cfg(test)]
#[path = "journal.test.rs"]
mod tests;
