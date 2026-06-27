//! Per-(provider, model) entry overrides.
//!
//! Sits in `ProviderConfig.models` as `BTreeMap<String, ProviderModelOverride>`.
//! Each entry supplies the per-(provider, model) routing fields
//! (`api_model_name`) and any per-entry `ModelInfo` overrides under an
//! explicit `overrides: { ... }` nesting — flattening would disable
//! `deny_unknown_fields` on the inner struct (per serde docs).

use crate::model::partial::PartialModelInfo;
use serde::Deserialize;
use serde::Serialize;

/// Wire format — every field optional. `BTreeMap` order is stable.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct PartialProviderModelOverride {
    /// Provider-side model name when it differs from `model_id`. For
    /// example, an internal gateway routes `internal/coder-v3` to
    /// `ep-internal-v3-prod` on the wire.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_model_name: Option<String>,
    /// Per-entry `ModelInfo` overrides (sampling, capabilities, …).
    /// Layered on top of the catalog `models.json` entry.
    #[serde(skip_serializing_if = "PartialModelInfo::is_empty")]
    pub overrides: PartialModelInfo,
}

/// Resolved per-(provider, model) override.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ProviderModelOverride {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_model_name: Option<String>,
    #[serde(skip_serializing_if = "PartialModelInfo::is_empty")]
    pub overrides: PartialModelInfo,
}

impl ProviderModelOverride {
    pub fn from_partial(partial: PartialProviderModelOverride) -> Self {
        Self {
            api_model_name: partial.api_model_name,
            overrides: partial.overrides,
        }
    }

    /// Apply the per-entry `ModelInfo` overrides on top of an
    /// accumulator. Used in `build_model_registry` after the catalog
    /// `models.json` layer.
    ///
    /// **Scope is `overrides` only.** `api_model_name` is a routing
    /// field stored separately on
    /// `ResolvedModel.provider_model.api_model_name` and consumed by
    /// `model_factory::build_*`; it is not part of the merged
    /// `ModelInfo`.
    pub fn apply_overrides_to(&self, acc: &mut PartialModelInfo) {
        acc.merge_from(&self.overrides);
    }
}
