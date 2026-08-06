//! Skill availability overrides and their persistence lock.
//!
//! Lives beside the other domain types rather than in the event layer:
//! `coco-config` resolves the override tiers and `coco-skills` enforces
//! them, neither of which has anything to do with wire events.

use serde::Deserialize;
use serde::Serialize;

/// Per-skill override state stored under `skill_overrides` in any
/// settings tier. Drives the `/skills` 4-state editor ladder.
///
/// Wire format is kebab-case (`"on"`, `"name-only"`,
/// `"user-invocable-only"`, `"off"`) — JSON settings files round-trip
/// without translation.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkillOverrideState {
    /// Default — full description in model listing, both user `/` and
    /// model Skill-tool invocation allowed.
    On,
    /// Name-only listing (model sees `- name` without description);
    /// **model can still invoke**. Saves description tokens.
    NameOnly,
    /// Hidden from model listing; Skill tool rejects model invocation
    /// **unless** the user typed `/<name>` in the current turn. Slash
    /// dispatcher still works.
    UserInvocableOnly,
    /// Fully disabled — hidden from listing AND `/` autocomplete;
    /// Skill tool rejects every invocation attempt.
    Off,
}

impl SkillOverrideState {
    /// Cycle to the next state in the TS 4-state ladder
    /// (`on → name-only → user-invocable-only → off → on`). Used by
    /// the `/skills` dialog Space key.
    pub const fn next(self) -> Self {
        match self {
            Self::On => Self::NameOnly,
            Self::NameOnly => Self::UserInvocableOnly,
            Self::UserInvocableOnly => Self::Off,
            Self::Off => Self::On,
        }
    }
}

/// Which precedence layer originated a non-overridable lock on a
/// skill's `skill_overrides` state. Mirrors the four `lock.source`
/// values returned by TS `oT5` (`cli_inner_pretty.js:476885-476893`).
///
/// In precedence order (highest first): [`Self::Policy`] →
/// [`Self::Flag`] → [`Self::Author`] → [`Self::Plugin`]. A lock means
/// the `/skills` dialog renders `🔒 <label>` for the row and refuses
/// to cycle it (Space is a no-op).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillLockSource {
    /// `policySettings.skill_overrides[name]` — enterprise-managed.
    Policy,
    /// `flagSettings.skill_overrides[name]` — `--settings <path>`
    /// invocation override.
    Flag,
    /// Skill frontmatter `disable-model-invocation: true` — author
    /// forced to `user-invocable-only`.
    Author,
    /// `skill.source == Plugin` — plugin-contributed skills are
    /// forced to `on` (manage via `/plugin` instead).
    Plugin,
}

/// A non-overridable lock on a skill row in the `/skills` dialog.
/// Carries both the originating tier ([`Self::source`]) and the
/// forced 4-state value ([`Self::forced_value`]) so downstream
/// renderers don't need to re-derive the value from per-tier maps.
///
/// TS mirror: `oT5` returns `{ value, source }` —
/// `cli_inner_pretty.js:476885-476893`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillLock {
    pub source: SkillLockSource,
    pub forced_value: SkillOverrideState,
}
