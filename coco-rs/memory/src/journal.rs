//! Memory-side learning-journal writes.
//!
//! Best-effort, host-side (post-fork) appends of memory events to the
//! append-only `memory-journal.jsonl` that sits as a sibling of the memdir —
//! outside both write-fence rings, so the extract/dream forks can never reach
//! it. Shared by `extract` (writes) and `dream` (consolidations).
//!
//! Deliberately NOT gated on `SkillLearnConfig::journal_enabled`, even though
//! `/journey` merges this journal with the skill one: both call sites are inside
//! `MemoryRuntime`, which is only built when `Feature::AutoMemory` is on, so the
//! subsystem gate already covers them. Reading a `skill_learn.*` key from here
//! would make memory depend on skill-learning's config to decide whether to
//! record its own facts.

use std::path::Path;

use coco_types::{JourneyEvent, JourneyRecord};

/// Append one memory event to the learning journal (best-effort). Blocking I/O
/// — the memory services already run this on their post-fork path.
///
/// `pub` because the `/journey` host writes `MemoryDeleted` here too: this
/// crate owns the memory journal's path derivation and envelope, so a second
/// caller must reuse it rather than re-derive them.
pub fn append_event(memdir: &Path, session_id: Option<&str>, event: JourneyEvent) {
    let now_ms = coco_utils_common::now_epoch_ms().unwrap_or(0);
    let journal = crate::path::memory_journal_path(memdir);
    coco_maintenance::journal::append_rotating(
        &journal,
        &JourneyRecord::new(now_ms, session_id.map(str::to_string), event),
    );
}
