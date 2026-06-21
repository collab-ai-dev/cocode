//! Per-agent project-shared memory snapshots.
//!
//! Project teams can ship a baseline `MEMORY.md` (and any other `.md`
//! files) for an agent type by committing them under
//! `<cwd>/.coco/agent-memory-snapshots/<agentType>/` plus a
//! `snapshot.json` carrying `{updatedAt: ISO8601}`. At session bootstrap
//! the agent-loader compares the snapshot timestamp against
//! `<scope>/agent-memory/<agentType>/.snapshot-synced.json` and:
//!
//! - `none`: no snapshot file → nothing to sync
//! - `initialize`: snapshot exists but no local memory yet → copy
//!   snapshot files into local memory dir
//! - `prompt-update`: snapshot is newer than the last synced version
//!   → caller decides whether to replace or just `markSynced`
//!
//! Resolution mirrors the `agent_memory` module's scope rules.

use std::path::Path;
use std::path::PathBuf;

use coco_types::MemoryScope;
use serde::Deserialize;
use serde::Serialize;

use crate::agent_memory::agent_memory_dir;

/// Subdirectory under `<cwd>/.coco/` where project snapshots live.
pub const SNAPSHOT_BASE: &str = "agent-memory-snapshots";

const SNAPSHOT_JSON: &str = "snapshot.json";
const SYNCED_JSON: &str = ".snapshot-synced.json";

/// Snapshot metadata file shape — only carries the snapshot's
/// `updatedAt` timestamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotMeta {
    /// ISO 8601 timestamp the snapshot was published.
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

/// Synced-marker file shape — records the snapshot timestamp the
/// local copy was initialised from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncedMeta {
    /// ISO 8601 timestamp of the snapshot we last synced from.
    #[serde(rename = "syncedFrom")]
    pub synced_from: String,
}

/// Action returned by [`check_agent_memory_snapshot`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotAction {
    /// No snapshot file exists — nothing to do.
    None,
    /// Snapshot exists but no local memory has been initialised.
    /// Caller should run [`initialize_from_snapshot`].
    Initialize { snapshot_timestamp: String },
    /// Snapshot is newer than the last sync — caller decides whether
    /// to [`replace_from_snapshot`] or [`mark_snapshot_synced`].
    PromptUpdate { snapshot_timestamp: String },
}

// `sanitize_agent_type_for_path` lives in `coco_paths::sanitize` —
// see `agent_memory.rs` for the shared import note.
use coco_paths::sanitize_agent_type_for_path;

/// Snapshot directory for a given agent type.
pub fn snapshot_dir_for_agent(agent_type: &str, cwd: &Path) -> PathBuf {
    cwd.join(".coco")
        .join(SNAPSHOT_BASE)
        .join(sanitize_agent_type_for_path(agent_type))
}

fn snapshot_json_path(agent_type: &str, cwd: &Path) -> PathBuf {
    snapshot_dir_for_agent(agent_type, cwd).join(SNAPSHOT_JSON)
}

fn synced_json_path(agent_type: &str, scope: MemoryScope, cwd: &Path, home: &Path) -> PathBuf {
    agent_memory_dir(agent_type, scope, cwd, home).join(SYNCED_JSON)
}

fn read_meta<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let body = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<T>(&body).ok()
}

fn write_meta<T: Serialize>(path: &Path, meta: &T) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string(meta)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, body)
}

/// Determine what action (if any) is needed to sync project-shared
/// snapshot state into the local agent memory dir.
pub fn check_agent_memory_snapshot(
    agent_type: &str,
    scope: MemoryScope,
    cwd: &Path,
    home: &Path,
) -> SnapshotAction {
    let Some(snapshot_meta) = read_meta::<SnapshotMeta>(&snapshot_json_path(agent_type, cwd))
    else {
        return SnapshotAction::None;
    };

    let local_dir = agent_memory_dir(agent_type, scope, cwd, home);
    let has_local_memory = match std::fs::read_dir(&local_dir) {
        Ok(entries) => entries.flatten().any(|e| {
            e.file_type().map(|t| t.is_file()).unwrap_or(false)
                && e.file_name().to_string_lossy().ends_with(".md")
        }),
        Err(_) => false,
    };

    if !has_local_memory {
        return SnapshotAction::Initialize {
            snapshot_timestamp: snapshot_meta.updated_at,
        };
    }

    let synced = read_meta::<SyncedMeta>(&synced_json_path(agent_type, scope, cwd, home));
    match synced {
        Some(synced) if synced.synced_from >= snapshot_meta.updated_at => SnapshotAction::None,
        _ => SnapshotAction::PromptUpdate {
            snapshot_timestamp: snapshot_meta.updated_at,
        },
    }
}

fn copy_snapshot_to_local(
    agent_type: &str,
    scope: MemoryScope,
    cwd: &Path,
    home: &Path,
) -> std::io::Result<()> {
    let snapshot_dir = snapshot_dir_for_agent(agent_type, cwd);
    let local_dir = agent_memory_dir(agent_type, scope, cwd, home);
    std::fs::create_dir_all(&local_dir)?;

    let entries = match std::fs::read_dir(&snapshot_dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) || name_str == SNAPSHOT_JSON {
            continue;
        }
        let body = std::fs::read_to_string(entry.path())?;
        std::fs::write(local_dir.join(&name), body)?;
    }
    Ok(())
}

/// First-time initialise: copy snapshot files into local memory dir
/// and record the synced timestamp.
pub fn initialize_from_snapshot(
    agent_type: &str,
    scope: MemoryScope,
    snapshot_timestamp: &str,
    cwd: &Path,
    home: &Path,
) -> std::io::Result<()> {
    copy_snapshot_to_local(agent_type, scope, cwd, home)?;
    mark_snapshot_synced(agent_type, scope, snapshot_timestamp, cwd, home)
}

/// Wipe existing `.md` files in the local memory dir, copy the
/// snapshot in, and mark synced.
pub fn replace_from_snapshot(
    agent_type: &str,
    scope: MemoryScope,
    snapshot_timestamp: &str,
    cwd: &Path,
    home: &Path,
) -> std::io::Result<()> {
    let local_dir = agent_memory_dir(agent_type, scope, cwd, home);
    if let Ok(entries) = std::fs::read_dir(&local_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) && name_str.ends_with(".md")
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    copy_snapshot_to_local(agent_type, scope, cwd, home)?;
    mark_snapshot_synced(agent_type, scope, snapshot_timestamp, cwd, home)
}

/// Record the snapshot timestamp as synced without changing local
/// memory contents. Used when the user opts to keep their local edits
/// over a newer project snapshot.
pub fn mark_snapshot_synced(
    agent_type: &str,
    scope: MemoryScope,
    snapshot_timestamp: &str,
    cwd: &Path,
    home: &Path,
) -> std::io::Result<()> {
    let path = synced_json_path(agent_type, scope, cwd, home);
    write_meta(
        &path,
        &SyncedMeta {
            synced_from: snapshot_timestamp.to_string(),
        },
    )
}

/// Boxed closure type returned by [`build_pending_inspector`]. Aliased
/// so the public signature reads cleanly and clippy doesn't trip on
/// `type_complexity`. Matches the shape of
/// `coco_subagent::SnapshotInspectorFn`.
pub type PendingSnapshotInspector = Box<dyn Fn(&str, MemoryScope) -> Option<String> + Send + Sync>;

/// Build the closure consumed by
/// `coco_subagent::AgentDefinitionStore::set_snapshot_inspector`. Each
/// invocation runs [`check_agent_memory_snapshot`] for the
/// `(agent_type, scope)` pair and returns the snapshot timestamp when
/// the local memory is still behind it (`PromptUpdate` /
/// `Initialize`). Returns `None` when the memory is already synced
/// (`None` action), so callers see drift only after the bootstrap
/// auto-sync runs.
///
/// The returned closure is `Send + Sync`; the captured paths are
/// cloned into the closure so the loader can run in any blocking-pool
/// task without lifetime coupling.
pub fn build_pending_inspector(cwd: PathBuf, home: PathBuf) -> PendingSnapshotInspector {
    Box::new(move |agent_type: &str, scope: MemoryScope| {
        match check_agent_memory_snapshot(agent_type, scope, &cwd, &home) {
            SnapshotAction::None => None,
            SnapshotAction::Initialize { snapshot_timestamp }
            | SnapshotAction::PromptUpdate { snapshot_timestamp } => Some(snapshot_timestamp),
        }
    })
}

#[cfg(test)]
#[path = "agent_memory_snapshot.test.rs"]
mod tests;
