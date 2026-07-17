//! Skill-side learning-journal writes.
//!
//! The agent journal (`<config_home>/skills/.agent-journal.jsonl`) is a sibling
//! of the fenced agent root, so the review fork can never reach it. Every skill
//! fact — learned/updated (stamp), retired/promoted (curator), and the manual
//! `/journey` mutations (host) — enters through here.
//!
//! This is the single write site on purpose: the journal's path derivation, its
//! `JourneyRecord` envelope, and its clock were previously re-derived at three
//! call sites across two crates, which is how the size cap drifted into four
//! separate constants.

use std::path::Path;

use coco_types::{JourneyEvent, JourneyRecord};

/// Append one skill fact to the learning journal (best-effort).
///
/// `session_id` is `None` for facts with no session context (the curator runs
/// on a timer; `/journey` mutations are host-side). Blocking I/O — call from
/// `spawn_blocking` in async contexts.
pub fn append_event(config_home: &Path, session_id: Option<&str>, event: JourneyEvent) {
    let now_ms = coco_utils_common::now_epoch_ms().unwrap_or(0);
    let journal = coco_skills::agent_scope::agent_journal_path(config_home);
    coco_maintenance::journal::append_rotating(
        &journal,
        &JourneyRecord::new(now_ms, session_id.map(str::to_string), event),
    );
}
