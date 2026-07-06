use std::collections::BTreeMap;
use std::collections::HashSet;

use coco_types::ModelSpec;
use coco_types::ProviderModelSelection;
use serde::Deserialize;
use serde::Serialize;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MoaPresetSettings {
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregator: Option<ProviderModelSelection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reference_models: Vec<ProviderModelSelection>,
    pub fanout: MoaFanout,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_max_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregator_temperature: Option<f32>,
}

impl Default for MoaPresetSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            aggregator: None,
            reference_models: Vec::new(),
            fanout: MoaFanout::PerIteration,
            reference_max_tokens: None,
            reference_temperature: None,
            aggregator_temperature: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MoaEndpointSpec {
    pub preset_name: String,
    pub aggregator: ModelSpec,
    pub reference_models: Vec<ModelSpec>,
    pub fanout: MoaFanout,
    pub reference_max_tokens: Option<i64>,
    pub reference_temperature: Option<f32>,
    pub aggregator_temperature: Option<f32>,
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

pub fn dedupe_specs(specs: Vec<ModelSpec>) -> Vec<ModelSpec> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for spec in specs {
        if seen.insert((spec.provider.clone(), spec.model_id.clone())) {
            out.push(spec);
        }
    }
    out
}
