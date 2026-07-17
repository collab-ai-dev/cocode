//! The agent-created skills scope — location-keyed loading + quarantine.
//!
//! Agent-written skills live under `<config_home>/skills/.agent/<name>/SKILL.md`,
//! the same root the skill-learning write fence confines the review fork to.
//! Everything under that root is treated as agent-authored **by location**:
//! the frontmatter is LLM-written and untrusted, so enforcement here overrides
//! whatever the file claims:
//!
//! - `provenance.origin` is force-stamped [`SkillOrigin::Agent`] (Curator
//!   eligibility + display), regardless of frontmatter.
//! - `allowed_tools` / `hooks` / `shell` are force-dropped — agent skills load
//!   inert and can never self-fire a shell, install hooks, or widen the
//!   permission set.
//! - `disable_model_invocation` is owned by the Curator's promotion state, not
//!   by the file: quarantined until promoted. Promotions live OUTSIDE the
//!   fenced root (`promotions_path`), so a prompt-injected review fork
//!   cannot self-promote what it writes. Users can still invoke quarantined
//!   skills via `/name` — that is what accrues the telemetry the Curator
//!   promotes (or retires) on.
//!
//! The parse-time neutralization in `parse_skill_markdown` (keyed on the
//! frontmatter `origin: agent`) remains as defense-in-depth for agent-stamped
//! files that get copied OUT of this directory; the location-keyed enforcement
//! here is the boundary that cannot be dodged by omitting frontmatter.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::{SkillDefinition, SkillManager, SkillOrigin};

/// Basename of the agent-owned skills directory under `<config_home>/skills`.
const AGENT_SKILLS_DIRNAME: &str = ".agent";

/// `<config_home>/skills` — parent of the fenced `.agent` root. Loop metadata
/// that must sit OUTSIDE the fence (the promotions store, the curator lock)
/// is placed here as a sibling of `.agent`; owning the geometry in one place
/// guarantees those artifacts can never silently move inside the fenced root.
pub fn skills_root(config_home: &Path) -> PathBuf {
    config_home.join("skills")
}

/// `<config_home>/skills/.agent` — the write-fence root for the skill-learning
/// loop and the only directory this scope loads from.
pub fn agent_skills_dir(config_home: &Path) -> PathBuf {
    skills_root(config_home).join(AGENT_SKILLS_DIRNAME)
}

/// `<config_home>/skills/.agent-promotions.json` — Curator-owned promotion
/// state. Deliberately a **sibling** of the `.agent` root, not inside it: the
/// review fork's write fence contains it to `.agent`, so promotion can only be
/// granted by trusted Rust code (the Curator), never by the fork itself.
fn promotions_path(config_home: &Path) -> PathBuf {
    skills_root(config_home).join(".agent-promotions.json")
}

/// `<config_home>/skills/.agent-journal.jsonl` — the append-only learning
/// journal for skill events. A **sibling** of the `.agent` root (same trick as
/// [`promotions_path`]): the review fork's write fence contains it to `.agent`
/// with a `[md,txt,json,yaml,yml,toml]` extension whitelist that denies
/// dot-prefixed names, so only trusted host-side Rust can append here.
pub fn agent_journal_path(config_home: &Path) -> PathBuf {
    skills_root(config_home).join(".agent-journal.jsonl")
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct PromotionsFile {
    #[serde(default)]
    promoted: Vec<String>,
}

/// Skill names promoted to model-invocability by the Curator.
pub fn load_promotions(config_home: &Path) -> HashSet<String> {
    let Ok(content) = std::fs::read_to_string(promotions_path(config_home)) else {
        return HashSet::new();
    };
    serde_json::from_str::<PromotionsFile>(&content)
        .map(|f| f.promoted.into_iter().collect())
        .unwrap_or_default()
}

/// Persist the full promotion set (sorted, atomic write). Returns `false`
/// when the write failed.
///
/// Blocking I/O — call from `spawn_blocking` in async contexts. Only the
/// Curator writes this file, under its cross-process O_EXCL lock; a curator
/// pass is idempotent and this write is self-healing (a dropped entry from an
/// unlikely same-process interleave is re-added by the next pass, since
/// telemetry is unchanged), so the unsynchronized read-modify-write is safe.
pub fn save_promotions(config_home: &Path, promoted: &HashSet<String>) -> bool {
    let mut names: Vec<String> = promoted.iter().cloned().collect();
    names.sort();
    let Ok(json) = serde_json::to_string_pretty(&PromotionsFile { promoted: names }) else {
        return false;
    };
    coco_utils_common::write_atomic(&promotions_path(config_home), json).is_ok()
}

/// Whether a scan includes Curator-retired (`disabled: true`) skills.
/// Two-variant enum (not a bool param) so call sites read unambiguously.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncludeDisabled {
    /// Include retired skills (journey: the timeline must show them).
    Yes,
    /// Skip retired skills (loader parity).
    No,
}

/// One agent skill as seen by a location-keyed scan: the enforced definition
/// plus the on-disk facts the Curator and `/journey` need for lifecycle
/// decisions and mutations.
#[derive(Debug, Clone)]
pub struct AgentSkillScan {
    /// Parsed definition with [`enforce_agent_quarantine`] applied (origin
    /// stamped, executable fields dropped, model-invocability from promotions).
    pub skill: SkillDefinition,
    /// Canonical `SKILL.md` path — the stable node identity + mutation target.
    pub skill_md: PathBuf,
    /// `disabled: true` in frontmatter (Curator-retired).
    pub disabled: bool,
    /// Promoted to model-invocability by the Curator.
    pub promoted: bool,
}

/// Location-keyed scan of every agent skill directory under
/// `<config_home>/skills/.agent`, parsing each `SKILL.md` and applying the same
/// quarantine enforcement as [`discover_agent_skills`], but WITHOUT the
/// disabled-skill filter that the normal discovery walk applies (so retired
/// skills are visible to `/journey`). Shared by the Curator and journey so
/// there is a single scan implementation.
///
/// Blocking I/O — call from `spawn_blocking` in async contexts.
pub fn scan_agent_skills(
    config_home: &Path,
    include_disabled: IncludeDisabled,
) -> Vec<AgentSkillScan> {
    let agent_root = agent_skills_dir(config_home);
    let promoted_set = load_promotions(config_home);
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&agent_root) else {
        return out;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        // Same case-insensitive lookup as the loader/curator.
        let Some(skill_md) = crate::find_skill_md(&dir) else {
            continue;
        };
        let Ok(mut skill) = crate::load_skill_from_file(&skill_md) else {
            continue;
        };
        let disabled = skill.disabled;
        if disabled && include_disabled == IncludeDisabled::No {
            continue;
        }
        let promoted = promoted_set.contains(&skill.name);
        enforce_agent_quarantine(&mut skill, promoted);
        out.push(AgentSkillScan {
            skill,
            skill_md,
            disabled,
            promoted,
        });
    }
    out
}

/// Discover, enforce, and register every agent-scope skill into `manager`.
///
/// Registered last among the disk scopes by convention, but shadowing is
/// enforced structurally rather than by order: `SkillCatalog::insert_disk`
/// lets any human-authored skill evict an agent one of the same name whenever
/// it registers, so an agent skill can never shadow a user / project / plugin /
/// legacy command even though plugins load after this call.
pub fn register_agent_skills(manager: &SkillManager, config_home: &Path) {
    for skill in discover_agent_skills(config_home) {
        manager.register(skill);
    }
}

/// Load every skill under `<config_home>/skills/.agent` with
/// `enforce_agent_quarantine` applied. Reuses the normal discovery walk
/// ([`crate::discover_skills`]), so disabled (Curator-retired) skills are
/// skipped and canonical-path dedup applies; quarantine never touches
/// `disabled`, so applying it after the walk is equivalent.
pub fn discover_agent_skills(config_home: &Path) -> Vec<SkillDefinition> {
    let mut skills = crate::discover_skills(&[agent_skills_dir(config_home)]);
    if skills.is_empty() {
        return skills;
    }
    let promoted = load_promotions(config_home);
    for skill in &mut skills {
        let is_promoted = promoted.contains(&skill.name);
        enforce_agent_quarantine(skill, is_promoted);
    }
    skills
}

/// Location-keyed enforcement for a skill loaded from the agent scope.
///
/// Overrides the parsed (LLM-authored, untrusted) frontmatter: origin is
/// stamped `Agent`, the executable-capability fields are dropped, and
/// model-invocability is granted solely by `promoted`. Private on purpose —
/// only the scope owner may decide the `promoted` flag.
fn enforce_agent_quarantine(skill: &mut SkillDefinition, promoted: bool) {
    skill.provenance.origin = SkillOrigin::Agent;
    skill.allowed_tools = None;
    skill.hooks = None;
    skill.shell = None;
    skill.disable_model_invocation = !promoted;
}

#[cfg(test)]
#[path = "agent_scope.test.rs"]
mod tests;
