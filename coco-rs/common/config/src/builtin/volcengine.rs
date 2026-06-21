//! Volcengine vendor catalog — `volcengine` provider only. Users
//! declare models against this provider via `~/.coco/providers.json`
//! or `~/.coco/models.json`.

use coco_types::ProviderApi;

use crate::model::partial::PartialModelInfo;
use crate::provider::PartialProviderConfig;

pub(super) fn providers() -> Vec<(&'static str, PartialProviderConfig)> {
    vec![(
        "volcengine",
        PartialProviderConfig {
            api: Some(ProviderApi::Volcengine),
            env_key: Some("ARK_API_KEY".into()),
            base_url: Some("https://ark.cn-beijing.volces.com/api/v3".into()),
            ..Default::default()
        },
    )]
}

pub(super) fn models() -> Vec<(&'static str, PartialModelInfo)> {
    Vec::new()
}
