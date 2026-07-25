use std::collections::BTreeMap;
use std::collections::HashSet;

use coco_types::ModelSpec;
use coco_types::ProviderModelSelection;
use coco_types::ReasoningEffort;
use serde::Deserialize;
use serde::Serialize;

use super::role_slots::RoleSlot;

pub const MOA_PROVIDER: &str = "moa";
pub const MAX_REFERENCE_MODELS: usize = 8;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MoaFanout {
    #[default]
    PerIteration,
    UserTurn,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MoaSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_preset: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub presets: BTreeMap<String, MoaPresetSettings>,
}

impl MoaSettings {
    pub fn default_preset_name(&self) -> &str {
        self.default_preset.as_deref().unwrap_or("default")
    }
}

/// One advisor slot in a MoA preset.
///
/// The JSON shape is deliberately identical to a `RoleSlot`
/// (`{provider, model_id, effort?}`) plus `enabled` — an advisor *is* a model
/// slot, so it configures like one.
///
/// **There are no `max_tokens` / `temperature` / `timeout_secs` fields here on
/// purpose.** Those are properties of the model, already declared per
/// `(provider, model_id)` on `ModelInfo` (and overridable via
/// `providers.<name>.models.<id>.overrides`). A slot-level copy would be a
/// fourth place to configure the same number. `effort` is the exception for the
/// same reason `RoleSlot` carries it: the identical model should be able to
/// reason differently depending on which slot it is serving.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoaReferenceSlot {
    pub provider: String,
    pub model_id: String,
    /// `false` parks the advisor without deleting it from the array. Parked
    /// slots are dropped at endpoint resolution, so they never reach fan-out
    /// and never count against [`MAX_REFERENCE_MODELS`].
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Per-slot reasoning intent, three-state exactly like `RoleSlot.effort`:
    /// `None` defers to the model's `default_thinking_level`,
    /// `Some(ReasoningEffort::Off)` explicitly suppresses thinking, anything
    /// else pins it. Advisors deliberately do not inherit the parent turn's
    /// thinking override, so this is their only control surface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<ReasoningEffort>,
}

fn default_enabled() -> bool {
    true
}

impl MoaReferenceSlot {
    pub fn selection(&self) -> ProviderModelSelection {
        ProviderModelSelection {
            provider: self.provider.clone(),
            model_id: self.model_id.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MoaPresetSettings {
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregator: Option<ProviderModelSelection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reference_models: Vec<MoaReferenceSlot>,
    pub fanout: MoaFanout,
    /// Per-reference wall-clock timeout, seconds. Bounds the whole retry loop
    /// (up to `MAX_REFERENCE_QUERY_ATTEMPTS` attempts), which is why it is not
    /// redundant with `ProviderConfig.timeout_secs` (a per-request bound). It
    /// sits at preset level because it constrains the fan-out barrier, not the
    /// model. `None` uses the built-in default (120 s), `0` disables the bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_timeout_secs: Option<i64>,
}

impl Default for MoaPresetSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            aggregator: None,
            reference_models: Vec::new(),
            fanout: MoaFanout::PerIteration,
            reference_timeout_secs: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MoaEndpointSpec {
    pub preset_name: String,
    pub aggregator: ModelSpec,
    /// Resolved, enabled advisors in declaration order. Reuses `RoleSlot`
    /// because that is exactly what an advisor is: a model plus the effort to
    /// use while it serves this slot.
    pub reference_models: Vec<RoleSlot<ModelSpec>>,
    pub fanout: MoaFanout,
    /// See `MoaPresetSettings::reference_timeout_secs`.
    pub reference_timeout_secs: Option<i64>,
}

impl MoaEndpointSpec {
    pub fn display_provider(&self) -> &'static str {
        MOA_PROVIDER
    }

    pub fn display_model_id(&self) -> &str {
        &self.preset_name
    }
}

pub fn is_moa_selection(selection: &ProviderModelSelection) -> bool {
    selection.provider == MOA_PROVIDER
}

/// Dedupe advisor slots by `(provider, model_id)`, first occurrence winning.
/// Querying the same model twice yields correlated guidance at double the
/// cost, so a duplicate is always a config mistake.
pub fn dedupe_reference_slots(slots: Vec<RoleSlot<ModelSpec>>) -> Vec<RoleSlot<ModelSpec>> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for slot in slots {
        if seen.insert((slot.model.provider.clone(), slot.model.model_id.clone())) {
            out.push(slot);
        }
    }
    out
}

/// Same dedupe, on the settings-facing shape (used by `coco moa configure`).
pub fn dedupe_reference_settings(slots: Vec<MoaReferenceSlot>) -> Vec<MoaReferenceSlot> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for slot in slots {
        if seen.insert((slot.provider.clone(), slot.model_id.clone())) {
            out.push(slot);
        }
    }
    out
}
