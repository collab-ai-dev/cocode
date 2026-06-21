//! Per-skill `skill_overrides` resolution.
//!
//! Three pure functions covering skill override resolution:
//!
//! - [`resolve_skill_override_lock`]
//! - [`resolve_skill_baseline`]
//! - [`effective_skill_state`]
//!
//! # Why three functions and not one
//!
//! Each is consumed at a different layer:
//!
//! - **Lock** ([`resolve_skill_override_lock`]) — drives the
//!   `/skills` dialog visualisation. A locked row renders
//!   `🔒 <state>` and is no-op on Space.
//! - **Baseline** ([`resolve_skill_baseline`]) — drives the dialog's
//!   diff-against-baseline save algorithm. When the user picks back
//!   the baseline value, the local override is deleted (`null` in
//!   the JSON patch) rather than written as a redundant override.
//! - **Effective state** ([`effective_skill_state`]) — drives the
//!   Skill tool gate and listing budget filter. Plugin source short-
//!   circuits to [`SkillOverrideState::On`].
//!
//! # `disable_model_invocation` is intentionally **not** in
//! [`effective_skill_state`]
//!
//! Does not check the author DMI flag. The Skill tool runs
//! a separate DMI gate, and the listing budget applies a separate
//! `XG$` predicate (`disable_model_invocation && state != "on"` →
//! skip). Folding DMI into [`effective_skill_state`] would cause an
//! author-DMI skill with no user override to silently disappear
//! from the listing — the model sees the name but can't auto-invoke.
//!
//! [`resolve_skill_override_lock`] **does** include DMI (as an
//! author lock) because the dialog visualisation is the only place
//! DMI's "locked to user-only" semantic matters.

use coco_config::SkillOverrideTiers;
use coco_types::SkillLock;
use coco_types::SkillLockSource;
use coco_types::SkillOverrideState;

use crate::SkillDefinition;
use crate::SkillSource;

/// Return the highest-precedence lock on a skill override, or `None`
/// if the user is free to edit it.
///
/// Precedence (highest first):
/// 1. `policySettings.skill_overrides[name]` → [`SkillLockSource::Policy`]
/// 2. `flagSettings.skill_overrides[name]` → [`SkillLockSource::Flag`]
/// 3. `skill.disable_model_invocation == true` → [`SkillLockSource::Author`]
///    (forced to [`SkillOverrideState::UserInvocableOnly`])
/// 4. `skill.source == Plugin` → [`SkillLockSource::Plugin`]
///    (forced to [`SkillOverrideState::On`])
pub fn resolve_skill_override_lock(
    skill: &SkillDefinition,
    tiers: &SkillOverrideTiers,
) -> Option<SkillLock> {
    if let Some(state) = tiers.policy.get(&skill.name).copied() {
        return Some(SkillLock {
            source: SkillLockSource::Policy,
            forced_value: state,
        });
    }
    if let Some(state) = tiers.flag.get(&skill.name).copied() {
        return Some(SkillLock {
            source: SkillLockSource::Flag,
            forced_value: state,
        });
    }
    if skill.disable_model_invocation {
        return Some(SkillLock {
            source: SkillLockSource::Author,
            forced_value: SkillOverrideState::UserInvocableOnly,
        });
    }
    if matches!(skill.source, SkillSource::Plugin { .. }) {
        return Some(SkillLock {
            source: SkillLockSource::Plugin,
            forced_value: SkillOverrideState::On,
        });
    }
    None
}

/// The project-or-user baseline used by the dialog's
/// diff-against-baseline save algorithm.
///
/// **Excludes** local / policy / flag tiers by design. The baseline
/// answers "if I delete this skill's key from
/// `.coco/settings.local.json`, what value resurfaces?" Local
/// itself is the layer being edited; policy / flag are locks
/// surfaced separately via [`resolve_skill_override_lock`].
pub fn resolve_skill_baseline(name: &str, tiers: &SkillOverrideTiers) -> SkillOverrideState {
    tiers
        .project
        .get(name)
        .or_else(|| tiers.user.get(name))
        .copied()
        .unwrap_or(SkillOverrideState::On)
}

/// The effective override state used by the Skill tool gate and
/// listing budget filter.
///
/// Plugin-sourced skills short-circuit to [`SkillOverrideState::On`]
/// (managed via `/plugin`, not `/skills`). Otherwise the highest-
/// precedence tier wins: `policy > flag > local > project > user`.
/// Default is [`SkillOverrideState::On`].
///
/// **Does not check `disable_model_invocation`** — see module
/// docs. The Skill tool calls this in addition to a separate DMI
/// gate.
pub fn effective_skill_state(
    skill: &SkillDefinition,
    tiers: &SkillOverrideTiers,
) -> SkillOverrideState {
    if matches!(skill.source, SkillSource::Plugin { .. }) {
        return SkillOverrideState::On;
    }
    tiers
        .policy
        .get(&skill.name)
        .or_else(|| tiers.flag.get(&skill.name))
        .or_else(|| tiers.local.get(&skill.name))
        .or_else(|| tiers.project.get(&skill.name))
        .or_else(|| tiers.user.get(&skill.name))
        .copied()
        .unwrap_or(SkillOverrideState::On)
}

#[cfg(test)]
#[path = "overrides.test.rs"]
mod tests;
