//! [`TraceWriter`] — append-only bundle writer (`manifest.json` + `trace.jsonl`).

use std::fs::File;
use std::io::Write;
use std::path::Path;

use coco_types::CoreEvent;
use coco_types::SessionId;
use serde::Deserialize;
use serde::Serialize;

use crate::error::IoSnafu;
use crate::error::Result;
use crate::error::SerdeSnafu;
use crate::event::TraceEvent;

/// Bundle format version. Bumped only on a breaking on-disk change.
pub const SCHEMA_VERSION: u32 = 1;
/// Manifest filename within a bundle directory.
pub const MANIFEST_FILE: &str = "manifest.json";
/// Events filename within a bundle directory (JSON Lines).
pub const EVENTS_FILE: &str = "trace.jsonl";

/// Bundle header. `created_unix_ms` is caller-supplied (not read from the wall
/// clock) so traces written in tests are deterministic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceManifest {
    pub schema_version: u32,
    pub session_id: SessionId,
    pub created_unix_ms: i64,
}

/// One line of `trace.jsonl`: a monotonic sequence number plus the event,
/// flattened so the wire shape is `{"seq":N,"kind":"…",…}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceRecord {
    pub seq: u64,
    #[serde(flatten)]
    pub event: TraceEvent,
}

/// Writes a session-trace bundle into a directory.
pub struct TraceWriter {
    manifest: TraceManifest,
    seq: u64,
    events_file: File,
}

impl TraceWriter {
    /// Create (or truncate) a bundle under `dir`, writing `manifest.json`
    /// immediately and opening `trace.jsonl` for append.
    pub fn create(
        dir: impl AsRef<Path>,
        session_id: impl Into<SessionId>,
        created_unix_ms: i64,
    ) -> Result<Self> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir).map_err(|e| {
            IoSnafu {
                message: format!("create dir {}: {e}", dir.display()),
            }
            .build()
        })?;
        let manifest = TraceManifest {
            schema_version: SCHEMA_VERSION,
            session_id: session_id.into(),
            created_unix_ms,
        };
        let manifest_json = serde_json::to_string_pretty(&manifest).map_err(|e| {
            SerdeSnafu {
                message: e.to_string(),
            }
            .build()
        })?;
        std::fs::write(dir.join(MANIFEST_FILE), manifest_json).map_err(|e| {
            IoSnafu {
                message: format!("write {MANIFEST_FILE}: {e}"),
            }
            .build()
        })?;
        let events_file = File::create(dir.join(EVENTS_FILE)).map_err(|e| {
            IoSnafu {
                message: format!("create {EVENTS_FILE}: {e}"),
            }
            .build()
        })?;
        Ok(Self {
            manifest,
            seq: 0,
            events_file,
        })
    }

    pub fn manifest(&self) -> &TraceManifest {
        &self.manifest
    }

    /// Number of records appended so far.
    pub fn recorded_count(&self) -> u64 {
        self.seq
    }

    /// Append a trace event.
    pub fn record(&mut self, event: TraceEvent) -> Result<()> {
        let record = TraceRecord {
            seq: self.seq,
            event,
        };
        let line = serde_json::to_string(&record).map_err(|e| {
            SerdeSnafu {
                message: e.to_string(),
            }
            .build()
        })?;
        writeln!(self.events_file, "{line}").map_err(|e| {
            IoSnafu {
                message: format!("append to {EVENTS_FILE}: {e}"),
            }
            .build()
        })?;
        self.seq += 1;
        Ok(())
    }

    /// Project `event` and append it if it carries durable semantics. Returns
    /// whether a record was written (`false` = non-durable, dropped).
    pub fn record_core(&mut self, event: &CoreEvent) -> Result<bool> {
        match TraceEvent::from_core_event(event) {
            Some(te) => {
                self.record(te)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }
}
