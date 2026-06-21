//! Unified `@` suggestion ranking and merging.
//!
//! Produces a single ranked list (cap 15) combining agents, file paths,
//! and MCP resources:
//!   - agents are scored by Fuse.js with weight 3 on `agentType`, weight 2
//!     on `displayText` — fuse scores are typically < 0.5 for decent
//!     matches, so agents dominate the top of the list versus the file
//!     prefix score (default 0.5)
//!   - files are pre-scored by nucleo (Rust) and merged below agents
//!   - MCP resources rank with the agent pool and keep at least one visible
//!     row when they match but agents fill the cap
//!
//! Rather than re-implement Fuse weighted scoring, we keep provider-local
//! ordering and merge through one cap layer. Agent rows still rank first,
//! but typed provider rows such as MCP resources are reserved space instead
//! of being dropped solely because the agent pool filled the cap.

use super::agent_search::AgentInfo;
use crate::completion::McpResourceCompletion;
use crate::widgets::suggestion_popup::SuggestionItem;
use crate::widgets::suggestion_popup::SuggestionMeta;

/// TS `MAX_UNIFIED_SUGGESTIONS` (`unifiedSuggestions.ts:70`).
const MAX_UNIFIED: usize = 15;

/// Build the agent half of the unified popup. Synchronous — agents are
/// already loaded in `session.available_agents` at session start.
///
/// Each row's label embeds the TS `displayText` format `"<name> (agent)"`
/// so the visual line shows the kind without a separate column. The
/// `SuggestionMeta::Agent { color }` carries the kind to the row
/// renderer (icon prefix) and to insertion (which strips the suffix
/// before splicing if needed — the submit parser at
/// `coco_context::user_input::extract_mentions:152` accepts the suffix
/// form directly, so we insert it verbatim).
pub fn seed_agent_items(agents: &[AgentInfo], query: &str) -> Vec<SuggestionItem> {
    let query_lower = query.to_lowercase();
    agents
        .iter()
        .filter(|a| query_lower.is_empty() || a.name.to_lowercase().contains(&query_lower))
        .map(|a| SuggestionItem {
            label: format!("{} (agent)", a.name),
            description: a.description.clone(),
            metadata: Some(SuggestionMeta::Agent { color: a.color }),
        })
        .collect()
}

pub fn seed_mcp_resource_items(
    resources: &[McpResourceCompletion],
    query: &str,
) -> Vec<SuggestionItem> {
    let query_lower = query.to_lowercase();
    resources
        .iter()
        .filter(|resource| {
            query_lower.is_empty()
                || resource.name.to_lowercase().contains(&query_lower)
                || resource.uri.to_lowercase().contains(&query_lower)
                || resource.server.to_lowercase().contains(&query_lower)
        })
        .map(|resource| SuggestionItem {
            label: resource.name.clone(),
            description: resource
                .description
                .clone()
                .or_else(|| Some(resource.uri.clone())),
            metadata: Some(SuggestionMeta::McpResource {
                server: resource.server.clone(),
                uri: resource.uri.clone(),
            }),
        })
        .collect()
}

/// Merge an async file-search result with already-seeded agent items.
///
/// Order: agents first (TS parity — agent fuse scores dominate file
/// scores), files appended. Total list is capped at [`MAX_UNIFIED`] so
/// the popup never overflows its vertical slot.
pub fn merge_file_results(
    mut seeded: Vec<SuggestionItem>,
    files: Vec<SuggestionItem>,
) -> Vec<SuggestionItem> {
    let room = MAX_UNIFIED.saturating_sub(seeded.len());
    seeded.extend(files.into_iter().take(room));
    seeded
}

pub fn merge_seeded_provider_items(
    agents: Vec<SuggestionItem>,
    mcp_resources: Vec<SuggestionItem>,
) -> Vec<SuggestionItem> {
    if agents.len() >= MAX_UNIFIED && !mcp_resources.is_empty() {
        let mut items = agents
            .into_iter()
            .take(MAX_UNIFIED.saturating_sub(1))
            .collect::<Vec<_>>();
        if let Some(resource) = mcp_resources.into_iter().next() {
            items.push(resource);
        }
        return items;
    }
    let mut items = agents;
    let room = MAX_UNIFIED.saturating_sub(items.len());
    items.extend(mcp_resources.into_iter().take(room));
    items
}

#[cfg(test)]
#[path = "unified.test.rs"]
mod tests;
