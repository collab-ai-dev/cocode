//! KAIROS daily log path + append semantics.
//!
//! Path layout: `<autoMemPath>/logs/YYYY/MM/YYYY-MM-DD.md`.
//! Append protocol: plain markdown append, one short timestamped
//! bullet per entry, no rewriting / no reorganisation.
//!
//! The compose-rendering side (the prompt that instructs the agent
//! to write a bullet) lives in [`crate::prompt`]. This module owns
//! the on-disk concern: path resolution and a small append helper
//! that creates parent directories on first write.

use std::path::{Path, PathBuf};

use coco_paths::ProjectPaths;
use tokio::io::AsyncWriteExt;

/// Resolve the KAIROS daily log path for `date` under
/// `project_paths`.
///
/// Convenience wrapper around [`ProjectPaths::daily_log`] —
/// here so callers that already have a `&MemoryDir` (and not a
/// `ProjectPaths`) can still go through one canonical helper by
/// constructing the relative tail explicitly.
pub fn daily_log_path(project_paths: &ProjectPaths, year: i32, month: u32, day: u32) -> PathBuf {
    project_paths.daily_log(year, month, day)
}

/// Compute the relative tail `logs/YYYY/MM/YYYY-MM-DD.md` given a
/// memory directory root that may have been resolved through an
/// override (e.g. `COCO_MEMORY_PATH_OVERRIDE`) and is not
/// reachable via a `ProjectPaths`.
///
/// The default coco-rs path goes through [`daily_log_path`]; this
/// helper exists for the override-supplied memory dir case (which
/// shows up in `crate::path::MemoryDir::resolve` when
/// `override_dir` is `Some`).
pub fn daily_log_path_under(memory_dir: &Path, year: i32, month: u32, day: u32) -> PathBuf {
    let yyyy = format!("{year:04}");
    let mm = format!("{month:02}");
    let dd = format!("{day:02}");
    memory_dir
        .join("logs")
        .join(&yyyy)
        .join(&mm)
        .join(format!("{yyyy}-{mm}-{dd}.md"))
}

/// Append-only writer for one project's KAIROS log.
///
/// Holds a `&ProjectPaths` reference (so callers can keep a long-
/// lived store without owning the paths themselves). On each append
/// it recomputes the date-keyed path and creates parent directories
/// lazily — creates the file (and parent directories) on first write.
pub struct DailyLogStore<'a> {
    project_paths: &'a ProjectPaths,
}

impl<'a> DailyLogStore<'a> {
    pub fn new(project_paths: &'a ProjectPaths) -> Self {
        Self { project_paths }
    }

    /// Append one line to today's log. The caller is responsible
    /// for the leading timestamp marker — per the prompt's "short
    /// timestamped bullet" instruction.
    ///
    /// Adds a trailing newline if `line` doesn't already end with
    /// one so the next append starts on its own line. Async because
    /// callers live in tokio contexts; doing blocking IO on the
    /// runtime would stall other tasks.
    pub async fn append(&self, line: &str, year: i32, month: u32, day: u32) -> std::io::Result<()> {
        let path = self.project_paths.daily_log(year, month, day);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let mut f = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;
        f.write_all(line.as_bytes()).await?;
        if !line.ends_with('\n') {
            f.write_all(b"\n").await?;
        }
        // tokio::fs::File drops via spawn_blocking, so the close may
        // outlive the function return. Explicit flush + sync_all
        // forces the bytes to disk before we hand control back —
        // matters for callers that immediately turn around and read
        // the file (and for tests doing the same).
        f.flush().await?;
        f.sync_all().await?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "daily_log.test.rs"]
mod tests;
