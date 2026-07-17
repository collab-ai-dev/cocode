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
//!
//! Same moment, the fork's skill events are appended to the append-only
//! learning journal (a `.agent-journal.jsonl` sibling of the fenced root that
//! the fork itself can never write): a SKILL.md whose `created-at` was absent
//! is a `SkillLearned`, one that already carried it is a `SkillUpdated`.

use std::path::Path;

use coco_skills::{SkillAuthor, SkillOrigin};
use coco_types::JourneyEvent;

use crate::notice::{SkillLearnNotice, SkillLearnVerb};

/// Hard backstop for the `description` frontmatter (byte budget, UTF-8-safe).
/// The review prompt asks for one sentence ≤ 60 chars; this only clips a
/// genuinely runaway description so the skill index doesn't burn context every
/// session. Generous enough to leave a normal 60-char (or multibyte) line
/// untouched.
const DESCRIPTION_MAX_BYTES: usize = 120;

/// Result of one stamping pass.
pub(crate) struct StampOutcome {
    /// Files whose provenance frontmatter was actually (re)written.
    pub stamped: usize,
    /// One user-visible notice per processed `SKILL.md`.
    pub notices: Vec<SkillLearnNotice>,
}

/// Everything one stamping pass needs. A struct rather than 8 positional
/// params so call sites stay readable.
pub(crate) struct StampRequest<'a> {
    pub agent_root: &'a Path,
    pub config_home: &'a Path,
    pub paths_written: &'a [std::path::PathBuf],
    pub now_rfc3339: &'a str,
    pub session_id: Option<&'a str>,
    pub author: SkillAuthor,
    /// Skill names that existed under `agent_root` **before** the fork ran,
    /// captured by trusted host code. This — not the fork-written `created-at`
    /// — decides Learned vs Updated: the frontmatter is LLM-authored, so a fork
    /// that writes its own `created-at` must not be able to disguise a birth as
    /// an update (which would drop the quarantine warning and leave the
    /// timeline with no creation event).
    pub pre_existing: &'a std::collections::HashSet<String>,
    /// Mirrors `SkillLearnConfig::journal_enabled`.
    pub journal_enabled: bool,
}

/// Stamp provenance into every fork-written `SKILL.md` under `agent_root`,
/// record a telemetry patch event per touched skill, and append a
/// `SkillLearned` / `SkillUpdated` fact per processed SKILL.md to the learning
/// journal. Blocking I/O — call from `spawn_blocking`.
pub(crate) fn stamp_written_skills(req: StampRequest<'_>) -> StampOutcome {
    let mut stamped = 0usize;
    let mut patched_skills: std::collections::HashSet<String> = std::collections::HashSet::new();
    // (skill name, was_newly_created) per SUCCESSFULLY processed SKILL.md.
    let mut journal_events: Vec<(String, bool)> = Vec::new();
    for path in req.paths_written {
        // The fence already confined writes to `agent_root`; the strip_prefix
        // both re-checks that and yields the `<skill>/...` relative path.
        let Ok(rel) = path.strip_prefix(req.agent_root) else {
            continue;
        };
        let skill_name = rel
            .components()
            .next()
            .and_then(|c| c.as_os_str().to_str())
            .map(str::to_string);
        if let Some(name) = &skill_name {
            patched_skills.insert(name.clone());
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
        let Some(name) = skill_name else {
            continue;
        };
        let was_new = !req.pre_existing.contains(&name);
        let updated = stamped_provenance(&content, req.now_rfc3339, req.author, was_new);
        // Only a skill whose provenance is actually on disk may be announced:
        // a failed write leaves the fork's (possibly spoofed) frontmatter in
        // place, so recording "learned" would claim an enforcement that never
        // happened.
        let stamp_persisted = match updated {
            Some(updated) => {
                // Atomic write so the 300ms skill hot-reload can never observe
                // a half-written file.
                match coco_utils_common::write_atomic(path, &updated) {
                    Ok(()) => {
                        stamped += 1;
                        true
                    }
                    Err(e) => {
                        tracing::warn!(
                            target: "coco_skill_learn::stamp",
                            path = %path.display(),
                            "provenance stamp failed; skipping journal + notice: {e}"
                        );
                        false
                    }
                }
            }
            // Already correct on disk — nothing to write, provenance holds.
            None => true,
        };
        if stamp_persisted {
            journal_events.push((name, was_new));
        }
    }
    for name in &patched_skills {
        coco_skills::telemetry::record_patch(req.config_home, name);
    }
    if req.journal_enabled {
        append_journal_events(req.config_home, req.session_id, &journal_events);
    }
    let notices = journal_events
        .into_iter()
        .map(|(name, was_new)| SkillLearnNotice {
            name,
            verb: if was_new {
                SkillLearnVerb::Learned
            } else {
                SkillLearnVerb::Updated
            },
        })
        .collect();
    StampOutcome { stamped, notices }
}

/// Skill names that currently have a directory under `agent_root`. Captured by
/// trusted host code *before* a review fork runs (see
/// [`StampRequest::pre_existing`]).
pub(crate) fn existing_skill_names(agent_root: &Path) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let Ok(entries) = std::fs::read_dir(agent_root) else {
        return out;
    };
    for entry in entries.flatten() {
        if entry.path().is_dir()
            && let Some(name) = entry.file_name().to_str()
        {
            out.insert(name.to_string());
        }
    }
    out
}

/// Append the collected skill events to the learning journal (best-effort).
fn append_journal_events(config_home: &Path, session_id: Option<&str>, events: &[(String, bool)]) {
    for (name, was_new) in events {
        let event = if *was_new {
            JourneyEvent::SkillLearned { name: name.clone() }
        } else {
            JourneyEvent::SkillUpdated { name: name.clone() }
        };
        crate::journal::append_event(config_home, session_id, event);
    }
}

/// Return the content with provenance enforced, or `None` when the frontmatter
/// is already correct.
///
/// `was_new` comes from the trusted host-side pre-fork snapshot, never from the
/// file. `origin` is force-set (an LLM-written `origin: user` is exactly the
/// spoof this exists to correct). `created-at` is likewise force-set on a birth
/// — a fork that supplied its own value cannot backdate the timeline — while an
/// UPDATE pass preserves the existing record (backfilling only if the fork
/// stripped it). All other keys round-trip untouched.
fn stamped_provenance(
    content: &str,
    now_rfc3339: &str,
    author: SkillAuthor,
    was_new: bool,
) -> Option<String> {
    use coco_skills::frontmatter_keys as keys;
    let fm = coco_frontmatter::parse(content);
    let mut obj = fm.data_to_json_map();
    let mut changed = false;
    let origin = serde_json::Value::String(SkillOrigin::Agent.as_str().to_string());
    if obj.get(keys::ORIGIN) != Some(&origin) {
        obj.insert(keys::ORIGIN.into(), origin);
        changed = true;
    }
    if was_new {
        // Birth: authorship + timestamp are ours to state, not the fork's.
        let created_by = serde_json::Value::String(author.as_str().to_string());
        if obj.get(keys::CREATED_BY) != Some(&created_by) {
            obj.insert(keys::CREATED_BY.into(), created_by);
            changed = true;
        }
        let created_at = serde_json::Value::String(now_rfc3339.to_string());
        if obj.get(keys::CREATED_AT) != Some(&created_at) {
            obj.insert(keys::CREATED_AT.into(), created_at);
            changed = true;
        }
    } else {
        // Update: keep the original record; only backfill what is missing.
        if !obj.contains_key(keys::CREATED_BY) {
            obj.insert(
                keys::CREATED_BY.into(),
                serde_json::Value::String(author.as_str().to_string()),
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
    }
    // Description budget backstop (L3): clip only a runaway description.
    let overlong = obj
        .get("description")
        .and_then(serde_json::Value::as_str)
        .filter(|d| d.len() > DESCRIPTION_MAX_BYTES)
        .map(str::to_string);
    if let Some(desc) = overlong {
        obj.insert(
            "description".into(),
            serde_json::Value::String(coco_utils_string::truncate_str(
                &desc,
                DESCRIPTION_MAX_BYTES,
            )),
        );
        changed = true;
    }
    changed.then(|| coco_frontmatter::emit_frontmatter(&obj, &fm.content))
}

#[cfg(test)]
#[path = "stamp.test.rs"]
mod tests;
