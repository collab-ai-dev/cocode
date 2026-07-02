//! Rust-side provenance stamping for fork-written skill files.
//!
//! The review prompt *asks* the LLM to include `origin: agent` /
//! `created-by` / `created-at` frontmatter, but nothing about an LLM is
//! enforceable — omission (or prompt-injected sabotage) must not leave an
//! unstamped artifact. Loading and curation are location-keyed and don't
//! trust these fields, so the stamp is audit metadata plus the trigger for
//! the parse-time defense-in-depth neutralization when a stamped file is
//! copied out of the agent directory. Stamping runs after every completed
//! review fork, in trusted code, over the exact paths the spawn driver
//! reported written.

use std::path::Path;

use coco_skills::{SkillAuthor, SkillOrigin};

/// Stamp provenance into every fork-written `SKILL.md` under `agent_root` and
/// record a telemetry patch event per touched skill. Returns the number of
/// files (re)stamped. Blocking I/O — call from `spawn_blocking`.
pub(crate) fn stamp_written_skills(
    agent_root: &Path,
    config_home: &Path,
    paths_written: &[std::path::PathBuf],
    now_rfc3339: &str,
) -> usize {
    let mut stamped = 0usize;
    let mut patched_skills: std::collections::HashSet<String> = std::collections::HashSet::new();
    for path in paths_written {
        // The fence already confined writes to `agent_root`; the strip_prefix
        // both re-checks that and yields the `<skill>/...` relative path.
        let Ok(rel) = path.strip_prefix(agent_root) else {
            continue;
        };
        if let Some(skill) = rel.components().next()
            && let Some(name) = skill.as_os_str().to_str()
        {
            patched_skills.insert(name.to_string());
        }
        let is_skill_md = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case("skill.md"));
        if !is_skill_md {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        if let Some(updated) = stamped_provenance(&content, now_rfc3339) {
            // Atomic write so the 300ms skill hot-reload can never observe a
            // half-written file.
            match coco_utils_common::write_atomic(path, &updated) {
                Ok(()) => stamped += 1,
                Err(e) => {
                    tracing::warn!(
                        target: "coco_skill_learn::stamp",
                        path = %path.display(),
                        "provenance stamp failed: {e}"
                    );
                }
            }
        }
    }
    for name in &patched_skills {
        coco_skills::telemetry::record_patch(config_home, name);
    }
    stamped
}

/// Return `content` with provenance frontmatter enforced, or `None` when it
/// is already correct. `origin` is force-set (an LLM-written `origin: user`
/// is exactly the spoof this exists to correct); `created-by` / `created-at`
/// are only filled when missing so an UPDATE pass preserves the original
/// authorship record. All other keys round-trip untouched.
fn stamped_provenance(content: &str, now_rfc3339: &str) -> Option<String> {
    use coco_skills::frontmatter_keys as keys;
    let fm = coco_frontmatter::parse(content);
    let mut obj = fm.data_to_json_map();
    let mut changed = false;
    let origin = serde_json::Value::String(SkillOrigin::Agent.as_str().to_string());
    if obj.get(keys::ORIGIN) != Some(&origin) {
        obj.insert(keys::ORIGIN.into(), origin);
        changed = true;
    }
    if !obj.contains_key(keys::CREATED_BY) {
        obj.insert(
            keys::CREATED_BY.into(),
            serde_json::Value::String(SkillAuthor::Review.as_str().to_string()),
        );
        changed = true;
    }
    if !obj.contains_key(keys::CREATED_AT) {
        obj.insert(
            keys::CREATED_AT.into(),
            serde_json::Value::String(now_rfc3339.to_string()),
        );
        changed = true;
    }
    changed.then(|| coco_frontmatter::emit_frontmatter(&obj, &fm.content))
}

#[cfg(test)]
#[path = "stamp.test.rs"]
mod tests;
