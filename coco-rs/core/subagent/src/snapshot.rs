//! Immutable per-turn view of the active agent catalog.
//!
//! Consumers (`AgentTool::prompt`, `/agents` commands, runtime spawn) take a
//! snapshot once and read from it without locking. Snapshots preserve the
//! order in which agents would be displayed (by source priority, then name).
//!
//! `AgentDefinitionStore::snapshot()` returns `Arc<AgentCatalogSnapshot>`,
//! so per-turn reads are pointer clones. Failed-load diagnostics live on
//! `AgentLoadReport`, not the snapshot — the snapshot is the *active*
//! catalog view, not a load journal.

use std::collections::BTreeMap;
use std::collections::HashSet;

use coco_types::AgentDefinition;

use crate::definition_store::LoadedAgentDefinition;

#[derive(Debug, Clone)]
pub struct AgentCatalogSnapshot {
    /// Active definitions keyed by `def.name` (the agent's display name = TS
    /// `agentType`, which is `frontmatter['name']` verbatim). For built-ins
    /// `def.name` equals the canonical `agent_type`; for a custom `.md` whose
    /// `name` is a non-canonical alias they differ — the map and every
    /// model-facing string (listing, deny filter, `find_active` lookup) key on
    /// `def.name`, so they stay mutually consistent. Do NOT switch this to
    /// `agent_type`: that would desync the lookup from the advertised listing.
    /// Alphabetically ordered via `BTreeMap` keying — deterministic across
    /// platforms and reload cycles. Note byte-wise lex order means
    /// PascalCase entries (`Explore`, `Plan`) sort before lowercase
    /// entries (`build`, `coco-guide`).
    active: BTreeMap<String, AgentDefinition>,
    /// Active `agent_type`s in source-load order (built-in → plugin →
    /// user → project → flag → managed; first-occurrence position within
    /// that, mirroring JS `Map` key-insertion semantics in
    /// `getActiveAgentsFromList`). Used only for prompt rendering so the
    /// model-visible "Available agent types" block — and its prompt-cache
    /// key — is deterministic. Lookups/counts use `active`.
    load_order: Vec<String>,
    /// All loaded definitions (including those overridden by higher-priority
    /// sources). Used by `/agents show` to display source chains.
    all: Vec<LoadedAgentDefinition>,
}

impl AgentCatalogSnapshot {
    pub fn new(active: BTreeMap<String, AgentDefinition>, all: Vec<LoadedAgentDefinition>) -> Self {
        let load_order = compute_load_order(&all, &active);
        Self {
            active,
            load_order,
            all,
        }
    }

    /// All active agents in deterministic (alphabetical) order. Used by
    /// lookups, counts, and any order-insensitive consumer.
    pub fn active(&self) -> impl Iterator<Item = &AgentDefinition> {
        self.active.values()
    }

    /// Active agents in source-load order. Prompt rendering uses this
    /// so the "Available agent types" block matches
    /// `getActiveAgentsFromList` (loadAgentsDir.ts:193-220).
    pub fn active_in_load_order(&self) -> impl Iterator<Item = &AgentDefinition> {
        self.load_order
            .iter()
            .filter_map(|name| self.active.get(name))
    }

    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    /// Look up an active agent by its `name` (= TS `agentType`; the value the
    /// model picks from the advertised listing). Keyed on `def.name`, not the
    /// canonicalized `agent_type` — see the `active` field docs.
    pub fn find_active(&self, name: &str) -> Option<&AgentDefinition> {
        self.active.get(name)
    }

    /// Every loaded definition (including overridden ones), in load order.
    pub fn all(&self) -> &[LoadedAgentDefinition] {
        &self.all
    }

    /// Active agents whose `required_mcp_servers` are all satisfied by
    /// the connected MCP server set. Definitions with no requirements
    /// pass through unchanged. Matching is case-insensitive substring
    /// (`loadAgentsDir.ts:hasRequiredMcpServers`).
    ///
    /// AgentTool's prompt-rendering layer should use this filter so the
    /// model never sees an agent it can't actually call — pre-filter
    /// gives a better error surface than the execute-time
    /// `check_mcp_ready` failure.
    pub fn active_with_mcp(
        &self,
        connected_servers: &[String],
    ) -> impl Iterator<Item = &AgentDefinition> {
        self.active().filter(move |def| {
            if def.required_mcp_servers.is_empty() {
                return true;
            }
            def.required_mcp_servers.iter().all(|pattern| {
                let needle = pattern.to_lowercase();
                connected_servers
                    .iter()
                    .any(|server| server.to_lowercase().contains(&needle))
            })
        })
    }
}

/// Compute the source-load order of active `agent_type`s. Mirrors
/// `getActiveAgentsFromList` (loadAgentsDir.ts:193-220): iterate sources
/// ascending by precedence (built-in → plugin → user → project → flag →
/// managed), and within a source in load order, recording each name at
/// its first occurrence (JS `Map` key-insertion position). Names not in
/// `active` (fully overridden) are skipped — the winning definition is
/// looked up via `active` at render time.
fn compute_load_order(
    all: &[LoadedAgentDefinition],
    active: &BTreeMap<String, AgentDefinition>,
) -> Vec<String> {
    let mut indexed: Vec<(u8, usize, &str)> = all
        .iter()
        .enumerate()
        .map(|(i, l)| {
            (
                l.definition.source.priority(),
                i,
                l.definition.name.as_str(),
            )
        })
        .collect();
    // Ascending priority, then stable original index within a source.
    indexed.sort_by_key(|(priority, idx, _)| (*priority, *idx));
    let mut order = Vec::new();
    let mut seen = HashSet::new();
    for (_, _, name) in indexed {
        if active.contains_key(name) && seen.insert(name) {
            order.push(name.to_owned());
        }
    }
    order
}

/// Pure predicate — `true` when every entry in
/// `def.required_mcp_servers` matches at least one connected server
/// (case-insensitive substring).
pub fn has_required_mcp_servers(def: &AgentDefinition, connected_servers: &[String]) -> bool {
    if def.required_mcp_servers.is_empty() {
        return true;
    }
    def.required_mcp_servers.iter().all(|pattern| {
        let needle = pattern.to_lowercase();
        connected_servers
            .iter()
            .any(|server| server.to_lowercase().contains(&needle))
    })
}

#[cfg(test)]
#[path = "snapshot.test.rs"]
mod tests;
