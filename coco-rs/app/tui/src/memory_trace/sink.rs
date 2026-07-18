//! Rotating JSONL sink and wire representation for memory diagnostics.

use std::ffi::OsString;
use std::fs::File;
use std::fs::OpenOptions;
use std::io;
use std::io::BufWriter;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::SystemTime;

use coco_utils_jemalloc::JemallocStats;
use serde::Serialize;

use super::Thresholds;
use crate::perf::MemoryPhase;
use crate::perf::MemorySampleKind;
use crate::perf::ProcessMemorySample;
use crate::perf::RetainedMemoryStats;

const STALE_FILE_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Debug)]
pub(super) struct MemoryTraceInner {
    pub(super) path: PathBuf,
    pub(super) writer: Option<BufWriter<File>>,
    pub(super) bytes_written: u64,
    pub(super) max_file_bytes: u64,
    pub(super) rotated_files: usize,
    pub(super) thresholds: Thresholds,
    pub(super) write_error_reported: bool,
}

impl MemoryTraceInner {
    pub(super) fn write(&mut self, event: &MemoryTraceEvent<'_>) {
        let result = self.write_inner(event);
        if let Err(err) = result
            && !self.write_error_reported
        {
            self.write_error_reported = true;
            tracing::warn!(
                target: "tui::perf::mem",
                %err,
                path = %self.path.display(),
                "memory trace write failed"
            );
        }
    }

    fn write_inner(&mut self, event: &MemoryTraceEvent<'_>) -> io::Result<()> {
        let mut record = serde_json::to_vec(event).map_err(io::Error::other)?;
        record.push(b'\n');
        if self.bytes_written > 0
            && self.bytes_written.saturating_add(record.len() as u64) > self.max_file_bytes
        {
            self.rotate()?;
        }
        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| io::Error::other("memory trace writer missing"))?;
        writer.write_all(&record)?;
        writer.flush()?;
        self.bytes_written = self.bytes_written.saturating_add(record.len() as u64);
        Ok(())
    }

    pub(super) fn rotate(&mut self) -> io::Result<()> {
        if let Some(mut writer) = self.writer.take() {
            writer.flush()?;
        }
        if self.rotated_files == 0 {
            if self.path.exists() {
                std::fs::remove_file(&self.path)?;
            }
        } else {
            for index in (1..=self.rotated_files).rev() {
                let from = if index == 1 {
                    self.path.clone()
                } else {
                    backup_path(&self.path, index - 1)
                };
                if !from.exists() {
                    continue;
                }
                let to = backup_path(&self.path, index);
                if to.exists() {
                    std::fs::remove_file(&to)?;
                }
                std::fs::rename(from, to)?;
            }
        }
        self.writer = Some(open_append(&self.path)?);
        self.bytes_written = 0;
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub(super) enum MemoryTraceEvent<'a> {
    Sample {
        timestamp_ms: i64,
        phase: MemoryPhase,
        sample_kind: MemorySampleKind,
        process: Option<ProcessTrace>,
        jemalloc: Option<JemallocTrace>,
        #[serde(skip_serializing_if = "Option::is_none")]
        retained: Option<RetainedTrace>,
        gauge_bytes: Option<u64>,
        next_threshold_bytes: u64,
    },
    ThresholdCrossing {
        timestamp_ms: i64,
        phase: MemoryPhase,
        gauge_bytes: u64,
        crossed_threshold_bytes: u64,
        next_threshold_bytes: u64,
        stats_print: Option<&'a str>,
        stats_print_truncated: bool,
    },
    Purge {
        timestamp_ms: i64,
        phase: MemoryPhase,
        before: Option<JemallocTrace>,
        after: Option<JemallocTrace>,
        resident_reclaimed_bytes: Option<u64>,
        duration_ms: u128,
        error: Option<&'a str>,
    },
}

#[derive(Serialize)]
pub(super) struct ProcessTrace {
    rss_bytes: u64,
    vsz_bytes: u64,
    physical_footprint_bytes: Option<u64>,
    physical_footprint_peak_bytes: Option<u64>,
    sample_ms: u128,
    source: &'static str,
}

impl From<ProcessMemorySample> for ProcessTrace {
    fn from(sample: ProcessMemorySample) -> Self {
        Self {
            rss_bytes: sample.rss_bytes,
            vsz_bytes: sample.vsz_bytes,
            physical_footprint_bytes: sample.physical_footprint_bytes,
            physical_footprint_peak_bytes: sample.physical_footprint_peak_bytes,
            sample_ms: sample.sample_ms,
            source: sample.source_label(),
        }
    }
}

#[derive(Serialize)]
pub(super) struct JemallocTrace {
    allocated_bytes: u64,
    active_bytes: u64,
    resident_bytes: u64,
    retained_bytes: u64,
}

impl From<JemallocStats> for JemallocTrace {
    fn from(stats: JemallocStats) -> Self {
        Self {
            allocated_bytes: stats.allocated,
            active_bytes: stats.active,
            resident_bytes: stats.resident,
            retained_bytes: stats.retained,
        }
    }
}

#[derive(Serialize)]
pub(super) struct RetainedTrace {
    message_history_payload_bytes: usize,
    transcript_cell_text_bytes: usize,
    tool_execution_bytes: usize,
    reasoning_metadata_bytes: usize,
    subagent_bytes: usize,
    last_markdown_bytes: usize,
    markdown_memo_cache_bytes: usize,
    history_replay_cache_bytes: usize,
    total_bytes: usize,
}

impl From<RetainedMemoryStats> for RetainedTrace {
    fn from(stats: RetainedMemoryStats) -> Self {
        Self {
            message_history_payload_bytes: stats.message_history_payload_bytes,
            transcript_cell_text_bytes: stats.transcript_cell_text_bytes,
            tool_execution_bytes: stats.tool_execution_bytes,
            reasoning_metadata_bytes: stats.reasoning_metadata_bytes,
            subagent_bytes: stats.subagent_bytes,
            last_markdown_bytes: stats.last_markdown_bytes,
            markdown_memo_cache_bytes: stats.markdown_memo_cache_bytes,
            history_replay_cache_bytes: stats.history_replay_cache_bytes,
            total_bytes: stats.retained_total_bytes(),
        }
    }
}

pub(super) fn open_append(path: &Path) -> io::Result<BufWriter<File>> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map(BufWriter::new)
}

pub(super) fn backup_path(path: &Path, index: usize) -> PathBuf {
    let mut name = path
        .file_name()
        .map_or_else(|| OsString::from("memtrace.jsonl"), OsString::from);
    name.push(format!(".{index}"));
    path.with_file_name(name)
}

pub(super) fn prune_directory(
    dir: &Path,
    current_pid: u32,
    max_files: usize,
    min_cap_delete_age: Duration,
    now: SystemTime,
) -> io::Result<()> {
    let current_prefix = format!("coco.{current_pid}.jsonl");
    let mut files = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            (name.starts_with("coco.")
                && name.contains(".jsonl")
                && !name.starts_with(&current_prefix))
            .then(|| {
                let modified = entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                (entry.path(), modified)
            })
        })
        .collect::<Vec<_>>();
    files.sort_by_key(|(_, modified)| std::cmp::Reverse(*modified));

    for (index, (path, modified)) in files.into_iter().enumerate() {
        let age = now.duration_since(modified).unwrap_or_default();
        if age >= STALE_FILE_AGE || (index >= max_files && age >= min_cap_delete_age) {
            match std::fs::remove_file(&path) {
                Ok(()) if tracing::enabled!(tracing::Level::DEBUG) => {
                    tracing::debug!(path = %path.display(), "pruned stale memory trace")
                }
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
    }
    Ok(())
}

pub(super) fn truncate_utf8(text: &str, max_bytes: usize) -> (&str, bool) {
    if text.len() <= max_bytes {
        return (text, false);
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], true)
}

pub(super) fn now_ms() -> i64 {
    coco_utils_common::now_epoch_ms().unwrap_or_default()
}
