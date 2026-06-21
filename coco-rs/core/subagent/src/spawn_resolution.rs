//! Spawn-time subagent identity resolution.
//!
//! Combines the definition and subagent type that determine a subagent's model identity
//! into a single typed [`SubagentSelection`]. Pure logic — no IO, no
//! tokio. Callers (coordinator's `SwarmAgentHandle`) thread the typed
//! selection into the child's `AgentQueryConfig`.
//!
//! ## Precedence
//!
//! ```text
//! model:  definition.model > None
//! role:   definition.model_role > role_for_builtin(SubagentType) > ModelRole::Subagent
//! ```
//!
//! - **`definition.model`** — the agent `.md` frontmatter `model:` field.
//!   Already normalized to lowercase / `"inherit"` by the frontmatter parser.
//! - **`definition.model_role`** — explicit role declaration on the
//!   agent definition (the `.md` frontmatter `model_role:` field).
//!   Wins over the `subagent_type` mapping.
//! - **`subagent_type`** — the built-in classifier. Maps to a role via
//!   [`crate::subagent_role::role_for_builtin`].
//!
//! The `model` override and `model_role` are independent: a subagent
//! can pin a concrete model id while still carrying its semantic role
//! (used for fallback chains, recovery policy, telemetry).

use coco_types::{AgentDefinition, AgentTypeId, LlmModelSelection, ModelRole};

use crate::subagent_role::resolve_subagent_role;

/// Resolved spawn-time identity for a subagent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentSelection {
    /// Explicit model override — the AgentTool `model` parameter or the
    /// agent definition's `model` frontmatter field. `None` means
    /// "no override; use whatever the role's primary `ModelSpec`
    /// resolves to".
    pub model: Option<String>,
    /// Resolved `ModelRole`. Falls back to `Subagent` for custom agents
    /// without a `model_role` declaration.
    pub model_role: ModelRole,
    /// Unified model routing selection for the execution factory.
    pub model_selection: LlmModelSelection,
}

/// Pure resolver — see module doc for precedence rules.
pub fn resolve_subagent_selection(
    definition: Option<&AgentDefinition>,
    subagent_type: Option<&AgentTypeId>,
) -> SubagentSelection {
    let model = definition.and_then(|d| d.model.clone());
    let model_role = resolve_subagent_role(definition, subagent_type);
    let model_selection =
        LlmModelSelection::from_model_and_role(model.as_deref(), Some(model_role));
    SubagentSelection {
        model,
        model_role,
        model_selection,
    }
}

#[cfg(test)]
#[path = "spawn_resolution.test.rs"]
mod tests;
