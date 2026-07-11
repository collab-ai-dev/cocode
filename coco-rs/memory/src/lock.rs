//! Memory's consolidation lock — a thin policy wrapper over the shared
//! [`coco_maintenance::MaintenanceLock`].
//!
//! Memory owns only the *filename* convention (`.consolidate-lock` inside the
//! memory directory); all the CAS / mtime-gate / RAII-rollback logic lives in
//! the substrate crate so it isn't duplicated across maintenance loops.

use std::path::Path;

use coco_maintenance::MaintenanceLock;

/// Lock file basename inside the memory directory.
pub const LOCK_FILENAME: &str = ".consolidate-lock";

/// Build the auto-dream consolidation lock for `memory_dir`.
pub fn consolidate_lock(memory_dir: &Path) -> MaintenanceLock {
    MaintenanceLock::new(memory_dir, LOCK_FILENAME)
}
