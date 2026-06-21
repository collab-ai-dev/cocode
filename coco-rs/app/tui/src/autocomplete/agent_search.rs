//! Agent search source for the unified `@` popup.
//!
//! Pure in-memory projection of `coco_subagent::LoadedAgentDefinition`
//! into the lighter [`AgentInfo`] form consumed by the suggestion
//! pipeline. The popup feeds these through
//! [`super::unified::seed_agent_items`] which appends the `(agent)`
//! suffix and stamps `SuggestionMeta::Agent { color }` so the renderer
//! can draw the icon + color and the insertion path can pick the right
//! splice format.

use coco_types::AgentColorName;
use coco_types::AgentDefinition;

/// A loaded agent definition projection used for `@` autocomplete.
///
/// Carries the minimum the popup needs to render and the insertion path needs
/// to emit a `(agent)` suffix splice. Full definitions stay in
/// `coco_subagent::AgentDefinitionStore`; this is a UI-side cache.
#[derive(Debug, Clone)]
pub struct AgentInfo {
    /// User-facing agent name (= TS `agentType`). Embedded in the
    /// `"<name> (agent)"` label by [`super::unified::seed_agent_items`].
    pub name: String,
    /// Single-line `whenToUse` description.
    pub description: Option<String>,
    /// Optional badge color picked from the eight-color palette.
    pub color: Option<AgentColorName>,
}

impl AgentInfo {
    /// Project the full `coco_types::AgentDefinition` down to the UI
    /// surface. Used by the session bootstrap (`tui_runner::run_tui`)
    /// and by `/agents reload` to seed `session.available_agents`.
    pub fn from_definition(def: &AgentDefinition) -> Self {
        Self {
            name: def.name.clone(),
            description: def.description.clone(),
            color: def.color,
        }
    }
}
