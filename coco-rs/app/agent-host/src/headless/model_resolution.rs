use super::*;

// ─── Model resolution ────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ResolvedMainModel {
    pub provider: String,
    pub provider_api: Option<coco_types::ProviderApi>,
    pub model_id: String,
    pub supports_prompt_cache: bool,
}

pub fn resolve_main_model(runtime_config: &coco_config::RuntimeConfig) -> ResolvedMainModel {
    use coco_types::ModelRole;

    if let Some(main_spec) = runtime_config.model_roles.get(ModelRole::Main) {
        let supports_prompt_cache = matches!(main_spec.api, coco_types::ProviderApi::Anthropic)
            && runtime_config
                .model_registry
                .resolve(&main_spec.provider, &main_spec.model_id)
                .is_some_and(|model| {
                    model
                        .info
                        .capabilities
                        .as_ref()
                        .is_some_and(|caps| caps.contains(&coco_types::Capability::PromptCache))
                });
        return ResolvedMainModel {
            provider: main_spec.provider.clone(),
            provider_api: Some(main_spec.api),
            model_id: main_spec.model_id.clone(),
            supports_prompt_cache,
        };
    }

    let model = MockModel::new();
    ResolvedMainModel {
        provider: model.provider().to_string(),
        provider_api: None,
        model_id: model.model_id().to_string(),
        supports_prompt_cache: false,
    }
}
