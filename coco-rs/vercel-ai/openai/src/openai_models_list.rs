//! Live `/models` listing for the OpenAI provider.
//!
//! Hits the codex backend (`…/backend-api/codex/models`, OAuth bearer) for a
//! ChatGPT-subscription provider and the platform (`/v1/models`, API key)
//! otherwise — the same base_url + headers as every other call. This is a
//! provider-network concern, so it lives here rather than in
//! `services/inference` (see the multi-provider boundary in the root CLAUDE.md).

use serde::Deserialize;

/// Neutral, provider-agnostic view of one model from a `/models` listing.
/// Consumed at the coco-rs boundary (post-login catalog refresh) to augment
/// the picker without leaking the raw provider response shape inward.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredModel {
    /// Canonical model id (e.g. `gpt-5-5`).
    pub id: String,
    /// Context window in tokens when the endpoint reports it. The codex
    /// backend does; the platform `/v1/models` does not (→ `None`).
    pub context_window: Option<i64>,
}

/// Raw `/models` response. The platform returns `{ "data": [...] }`; the codex
/// backend has been observed to key the list under `models`. Accept both and
/// prefer `data`.
#[derive(Debug, Deserialize)]
pub(crate) struct ModelsListResponse {
    #[serde(default)]
    data: Vec<ModelListEntry>,
    #[serde(default)]
    models: Vec<ModelListEntry>,
}

#[derive(Debug, Deserialize)]
struct ModelListEntry {
    /// `id` on the platform; `slug` / `model` on some codex shapes.
    #[serde(alias = "slug", alias = "model")]
    id: Option<String>,
    /// `context_window` on codex; `context_length` on some shapes.
    #[serde(default, alias = "context_length")]
    context_window: Option<i64>,
}

impl ModelsListResponse {
    /// Flatten to neutral entries, dropping rows without an id. Prefers `data`,
    /// falling back to `models` when `data` is empty.
    pub(crate) fn into_discovered(self) -> Vec<DiscoveredModel> {
        let rows = if self.data.is_empty() {
            self.models
        } else {
            self.data
        };
        rows.into_iter()
            .filter_map(|e| {
                e.id.map(|id| DiscoveredModel {
                    id,
                    context_window: e.context_window,
                })
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "openai_models_list.test.rs"]
mod tests;
