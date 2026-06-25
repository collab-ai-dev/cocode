//! Resume journal for Dynamic Workflows: the append-only `journal.jsonl` and the
//! in-memory replay cache that back [`WorkflowHost::cached_agent_result`] /
//! [`WorkflowHost::record_agent_result`].
//!
//! On a fresh run the cache is empty, so every `agent()` call misses and runs
//! normally; each completed result is appended as a `result` line. On a resume
//! (`resumeFromRunId`) the cache is hydrated from the prior run's journal, so
//! the engine replays the longest unchanged prefix of `agent()` results without
//! re-spawning, then runs the diverged tail (the engine's per-run `diverged`
//! cursor stops consulting the cache after the first miss).
//!
//! Hashing lives here (host side) so the engine crate stays crypto-free, exactly
//! as [`coco_workflow_runtime::AgentCacheKey`] documents. The key is
//! `"<VERSION>:" + sha256(phase \0 prompt \0 canonical_opts)`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use coco_workflow_runtime::AgentCacheKey;
use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;

/// Cache-key schema version. Bump when the hashed-tuple layout or
/// canonicalization changes so stale journals never produce false hits.
const JOURNAL_KEY_VERSION: &str = "wfj1";

/// NUL separator between the hashed tuple fields (CC `journalKey`).
const FIELD_SEP: u8 = 0;

/// One append-only `journal.jsonl` record. `started` is written before a run
/// begins; `result` after it completes. Replay applies last-write-wins on
/// `result` records keyed by `key`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JournalEntry {
    /// Marker that an `agent()` call with this key is about to run.
    Started { key: String },
    /// A completed `agent()` result for this key (the cached value to replay).
    Result {
        key: String,
        value: serde_json::Value,
    },
}

/// The journal file path that sits alongside a run's `.output` file:
/// `<task_id>.journal.jsonl` (the output path is `<task_id>.output`). `None`
/// when the output path has no usable stem.
pub fn journal_path_for_output(output_path: &std::path::Path) -> Option<PathBuf> {
    // `<task_id>.output` → `<task_id>.journal.jsonl`; replace only the final
    // extension so a task id containing dots is preserved.
    Some(output_path.with_extension("journal.jsonl"))
}

/// The persisted-script path that sits alongside a run's `.output` file:
/// `<task_id>.workflow.js`. Mirrors the write site in the task runtime's
/// `register_workflow_task`, which persists each invocation's source so a later
/// `resumeFromRunId` can re-run the AUTHORITATIVE on-disk source.
pub fn script_path_for_output(output_path: &std::path::Path) -> PathBuf {
    output_path.with_extension("workflow.js")
}

/// Compute the resume cache key: `"<VERSION>:" + hex(sha256(phase \0 prompt \0
/// canonical_opts))`. Mirrors CC's `journalKey`.
pub fn journal_key(key: &AgentCacheKey) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.phase_title.as_bytes());
    hasher.update([FIELD_SEP]);
    hasher.update(key.prompt.as_bytes());
    hasher.update([FIELD_SEP]);
    hasher.update(key.canonical_opts.as_bytes());
    let digest = hasher.finalize();
    format!("{JOURNAL_KEY_VERSION}:{digest:x}")
}

/// In-memory replay cache + append-only journal file. Cheap to clone-share via
/// `Arc`; the cache is mutex-protected, the path is immutable after build.
pub struct WorkflowJournal {
    /// hash → cached result value. Hydrated from disk on resume; populated as
    /// the live run records results.
    cache: Mutex<HashMap<String, serde_json::Value>>,
    /// `journal.jsonl` path. `None` disables persistence (cache-only).
    path: Option<PathBuf>,
}

impl WorkflowJournal {
    /// A live-run journal: empty cache, results appended to `path` (when set).
    pub fn new(path: Option<PathBuf>) -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
            path,
        }
    }

    /// A resume journal: hydrate the cache from `source` (the prior run's
    /// `journal.jsonl`), then continue appending to `path` (the new run's
    /// journal). `source` and `path` may be the same file when resuming in
    /// place. Hydration is best-effort: a missing/corrupt source yields an empty
    /// cache, so resume degrades to a fresh run rather than failing.
    pub fn resumed(source: &std::path::Path, path: Option<PathBuf>) -> Self {
        let cache = Mutex::new(hydrate_from_disk(source));
        Self { cache, path }
    }

    /// Replay lookup: `Some(value)` on a cache hit. On a miss, append a
    /// `started` marker (best-effort) so a later resume can see the call was
    /// attempted, mirroring CC's "started before run".
    pub async fn lookup(&self, key: &AgentCacheKey) -> Option<serde_json::Value> {
        let hash = journal_key(key);
        let hit = self
            .cache
            .lock()
            .ok()
            .and_then(|cache| cache.get(&hash).cloned());
        if hit.is_none() {
            self.append(JournalEntry::Started { key: hash }).await;
        }
        hit
    }

    /// Record a completed result: update the in-memory cache and append a
    /// `result` line. Null results are skipped (nothing to replay), matching CC.
    pub async fn record(&self, key: &AgentCacheKey, value: &serde_json::Value) {
        if value.is_null() {
            return;
        }
        let hash = journal_key(key);
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(hash.clone(), value.clone());
        }
        self.append(JournalEntry::Result {
            key: hash,
            value: value.clone(),
        })
        .await;
    }

    /// Append one JSONL line to the journal file (best-effort: a write failure is
    /// logged, not propagated — the in-memory cache still serves this run).
    async fn append(&self, entry: JournalEntry) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
        let Ok(mut line) = serde_json::to_string(&entry) else {
            return;
        };
        line.push('\n');
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        match tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
        {
            Ok(mut file) => {
                use tokio::io::AsyncWriteExt;
                // `write_all` only guarantees the bytes are *scheduled* on the
                // blocking pool; `flush` awaits the in-flight write so a
                // subsequent read-back (a cross-run resume hydrating this
                // journal) is guaranteed to observe the line. Without the flush
                // the hydrate races the write under full-suite load.
                let write = async {
                    file.write_all(line.as_bytes()).await?;
                    file.flush().await
                };
                if let Err(error) = write.await {
                    tracing::warn!(
                        target: "coco::workflow",
                        %error,
                        path = %path.display(),
                        "failed to append workflow journal line"
                    );
                }
            }
            Err(error) => {
                tracing::warn!(
                    target: "coco::workflow",
                    %error,
                    path = %path.display(),
                    "failed to open workflow journal for append"
                );
            }
        }
    }
}

/// Read an existing `journal.jsonl`, applying last-write-wins for `result`
/// entries (keyed by hash). `started` lines are ignored for hydration — only
/// completed results are replayable. Unparsable lines are skipped.
fn hydrate_from_disk(path: &std::path::Path) -> HashMap<String, serde_json::Value> {
    let mut cache = HashMap::new();
    let Ok(contents) = std::fs::read_to_string(path) else {
        return cache;
    };
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(JournalEntry::Result { key, value }) = serde_json::from_str::<JournalEntry>(line)
        {
            // Last write wins: later lines overwrite earlier ones for the same
            // key.
            cache.insert(key, value);
        }
    }
    cache
}

#[cfg(test)]
#[path = "workflow_journal.test.rs"]
mod tests;
