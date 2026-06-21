//! Zai vendor catalog — `zai` provider only. Users declare models
//! against this provider via `~/.coco/providers.json` or
//! `~/.coco/models.json`.

use coco_types::ProviderApi;

use crate::model::partial::PartialModelInfo;
use crate::provider::PartialProviderConfig;

pub(super) fn providers() -> Vec<(&'static str, PartialProviderConfig)> {
    vec![(
        "zai",
        PartialProviderConfig {
            api: Some(ProviderApi::Zai),
            env_key: Some("ZAI_API_KEY".into()),
            base_url: Some("https://api.z.ai/v1".into()),
            ..Default::default()
        },
    )]
}

pub(super) fn models() -> Vec<(&'static str, PartialModelInfo)> {
    Vec::new()
}
