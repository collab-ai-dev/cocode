//! Pure mapping from subagent identity → `ModelRole`.
//!
//! Resolution order (highest priority first):
//! 1. `AgentDefinition.model_role` (declared in frontmatter)
//! 2. Built-in `SubagentType` → role:
//!    - `Explore` → `ModelRole::Explore`
//!    - `Plan` → `ModelRole::Plan`
//!    - `Verification` → `ModelRole::Review`
//!    - `GeneralPurpose` / `StatusLine` / `CocoGuide` → `ModelRole::Subagent`
//! 3. Custom agent (no built-in mapping) → `ModelRole::Subagent`
//!
//! coco-rs adds role routing so `~/.coco/config.json` can map a role to
//! whichever (provider, model) is currently best, and `.md` agents declare
//! a role rather than a concrete model.
//!
//! This module has no dependencies beyond `coco_types` and is safe to
//! call from any layer. The companion `spawn_resolution` module in the
//! same crate combines this with the explicit `model` override and the
//! `subagent_type` to produce the final spawn-time selection.

use coco_types::{AgentDefinition, AgentTypeId, ModelRole, SubagentType};

/// Map a built-in `SubagentType` to its default `ModelRole`.
///
/// The mapping reflects the *intent* of each built-in: investigative
/// agents ride the `Explore` role (small + fast), planners ride `Plan`,
/// verification rides `Review`. Everything else lands on the generic
/// `Subagent` role.
pub const fn role_for_builtin(t: SubagentType) -> ModelRole {
    match t {
        SubagentType::Explore => ModelRole::Explore,
        SubagentType::Plan => ModelRole::Plan,
        SubagentType::Verification => ModelRole::Review,
        SubagentType::GeneralPurpose | SubagentType::StatusLine | SubagentType::CocoGuide => {
            ModelRole::Subagent
        }
    }
}

/// Resolve the `ModelRole` to use for spawning a subagent.
///
/// `definition` is consulted first (its `model_role` field wins if set).
/// Failing that, `subagent_type` selects via `role_for_builtin`. Custom
/// agents and `None` inputs fall through to `ModelRole::Subagent`.
pub fn resolve_subagent_role(
    definition: Option<&AgentDefinition>,
    subagent_type: Option<&AgentTypeId>,
) -> ModelRole {
    if let Some(role) = definition.and_then(|d| d.model_role) {
        return role;
    }
    match subagent_type {
        Some(AgentTypeId::Builtin(t)) => role_for_builtin(*t),
        Some(AgentTypeId::Custom(_)) | None => ModelRole::Subagent,
    }
}

#[cfg(test)]
#[path = "subagent_role.test.rs"]
mod tests;
