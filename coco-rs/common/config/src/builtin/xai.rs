//! xAI (Grok) vendor catalog — the `xai` provider only. Users declare Grok
//! models against this provider via `config home/providers.json` or
//! `config home/models.json`.
//!
//! `api: Xai` routes every instance name to the dedicated `vercel-ai-xai`
//! adapter and its canonical `"xai"` provider-options namespace.

use coco_types::OAuthFlowId;
use coco_types::ProviderApi;
use coco_types::WireApi;

use crate::model::partial::PartialModelInfo;
use crate::provider::PartialProviderConfig;

/// Canonical name of the builtin xAI API-key provider instance.
pub const XAI_PROVIDER: &str = "xai";
/// Canonical Grok-subscription provider instance.
pub const GROK_PROVIDER: &str = "grok";

pub(super) fn providers() -> Vec<(&'static str, PartialProviderConfig)> {
    vec![
        (
            XAI_PROVIDER,
            PartialProviderConfig {
                api: Some(ProviderApi::Xai),
                env_key: Some("XAI_API_KEY".into()),
                // OpenAI-compatible endpoint — the SDK appends `/chat/completions`.
                base_url: Some("https://api.x.ai/v1".into()),
                ..Default::default()
            },
        ),
        (
            GROK_PROVIDER,
            PartialProviderConfig {
                api: Some(ProviderApi::Xai),
                auth: Some(crate::provider::ProviderAuth::OAuth {
                    flow: OAuthFlowId::XaiGrok,
                }),
                wire_api: Some(WireApi::Responses),
                base_url: Some("https://cli-chat-proxy.grok.com/v1".into()),
                ..Default::default()
            },
        ),
    ]
}

pub(super) fn models() -> Vec<(&'static str, PartialModelInfo)> {
    Vec::new()
}
