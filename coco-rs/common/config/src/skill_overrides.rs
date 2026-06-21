//! Per-tier `skill_overrides` resolution surface.
//!
//! # Why this module exists
//!
//! Unlike every other field in [`crate::settings::Settings`], the
//! `skill_overrides` map cannot be flat-merged across tiers. Each
//! tier carries semantically distinct meaning that downstream
//! resolvers in `coco-skills` need to inspect independently:
//!
//! - `policy` / `flag` → non-overridable **locks**
//! - `project` / `user` → editable **baseline** (TS `aT5(name) =
//!   projectSettings.skill_overrides[name] ?? userSettings
//!   .skill_overrides[name]`)
//! - `local` → user's `/skills` dialog writes land here; diff against
//!   the baseline determines what gets persisted
//!
//! `RuntimeConfig` therefore exposes [`SkillOverrideTiers`] —
//! five independent maps preserved per-tier — instead of the merged
//! single map you would get from naive `Settings` merge.
//!
//! TS parity: `oT5` (`cli_inner_pretty.js:476885-476893`) reads each
//! tier individually via `v8("policySettings")?.skillOverrides`,
//! `v8("flagSettings")?.skillOverrides`, etc. The resolution rules
//! live in `coco-skills::overrides` (`oT5`, `aT5`, `st` mirrors).
//!
//! # Invariant
//!
//! **Do not merge.** Code that needs the merged view is acting on
//! incomplete information — every consumer in coco-rs must use the
//! three resolvers from `coco-skills::overrides` instead. The
//! `Settings.skill_overrides` field exists only so per-tier JSON
//! files parse cleanly; its merged value is unused by gates and
//! dialog payload construction.

use std::collections::BTreeMap;

use coco_types::SkillOverrideState;
use serde_json::Value;

use crate::settings::SettingSource;

/// Per-tier `skill_overrides` maps preserved without merging.
///
/// Populated at [`crate::RuntimeConfig`] build time from
/// `SettingsWithSource::per_source` raw JSON. Each map is keyed by
/// skill name; absent keys are equivalent to "no opinion from this
/// tier."
///
/// The plugin tier from coco-rs's six-layer `SettingSource` is not
/// represented here — plugin-contributed skills get their lock via
/// [`coco_types::SkillLockSource::Plugin`] computed from the skill's
/// `SkillSource::Plugin` variant, not from a settings layer.
#[derive(Debug, Clone, Default)]
pub struct SkillOverrideTiers {
    /// Highest-precedence lock layer. TS `policySettings`.
    pub policy: BTreeMap<String, SkillOverrideState>,
    /// CLI-flag override lock layer. TS `flagSettings`.
    pub flag: BTreeMap<String, SkillOverrideState>,
    /// Project shared `.coco/settings.json`. Editable baseline
    /// (used by the dialog's diff-against-baseline save).
    pub project: BTreeMap<String, SkillOverrideState>,
    /// User-global `~/.coco/settings.json` or `~/.claude/settings.json`.
    /// Editable baseline fallback.
    pub user: BTreeMap<String, SkillOverrideState>,
    /// Project-local `.coco/settings.local.json` — the dialog's
    /// write destination. Highest-precedence among editable layers.
    pub local: BTreeMap<String, SkillOverrideState>,
}

impl SkillOverrideTiers {
    /// Extract per-tier `skill_overrides` from a `per_source` raw JSON
    /// map. Tier entries absent from `per_source` produce empty maps;
    /// malformed values (wrong JSON shape or unrecognised state
    /// strings) are silently dropped per-skill — best-effort parsing
    /// keeps the rest of `/skills` working even when one file has a
    /// typo. The dialog's validation surfaces unknown skill names at
    /// edit time.
    pub fn from_per_source(per_source: &std::collections::HashMap<SettingSource, Value>) -> Self {
        Self {
            policy: extract_tier(per_source, SettingSource::Policy),
            flag: extract_tier(per_source, SettingSource::Flag),
            project: extract_tier(per_source, SettingSource::Project),
            user: extract_tier(per_source, SettingSource::User),
            local: extract_tier(per_source, SettingSource::Local),
        }
    }

    /// Whether every tier is empty — convenience for the "nothing
    /// configured" fast path (PR2 gates short-circuit to default
    /// `on` when this is true).
    pub fn is_empty(&self) -> bool {
        self.policy.is_empty()
            && self.flag.is_empty()
            && self.project.is_empty()
            && self.user.is_empty()
            && self.local.is_empty()
    }
}

fn extract_tier(
    per_source: &std::collections::HashMap<SettingSource, Value>,
    source: SettingSource,
) -> BTreeMap<String, SkillOverrideState> {
    let Some(root) = per_source.get(&source) else {
        return BTreeMap::new();
    };
    let Some(map_value) = root.get("skill_overrides") else {
        return BTreeMap::new();
    };
    let Value::Object(map) = map_value else {
        return BTreeMap::new();
    };
    map.iter()
        .filter_map(|(name, val)| {
            serde_json::from_value::<SkillOverrideState>(val.clone())
                .ok()
                .map(|state| (name.clone(), state))
        })
        .collect()
}

#[cfg(test)]
#[path = "skill_overrides.test.rs"]
mod tests;
