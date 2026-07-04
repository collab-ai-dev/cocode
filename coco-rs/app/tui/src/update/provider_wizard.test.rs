use super::*;
use crate::state::ProviderTemplate;
use crate::state::WizardTextField;
use coco_types::ProviderApi;
use coco_types::WireApi;

fn catalog_tpl() -> ProviderTemplate {
    ProviderTemplate {
        name: "deepseek-openai".to_string(),
        api: ProviderApi::OpenaiCompat,
        base_url: "https://api.deepseek.com/v1".to_string(),
        wire_api: WireApi::Chat,
        env_key: "DEEPSEEK_API_KEY".to_string(),
        is_custom: false,
    }
}

#[test]
fn advance_skips_base_url_for_catalog_template() {
    let mut w = ProviderWizardState::new(vec![catalog_tpl(), ProviderTemplate::custom()]);
    assert_eq!(w.step, ProviderWizardStep::Template);
    assert!(w.advance());
    assert_eq!(w.step, ProviderWizardStep::Name);
    assert!(w.advance());
    // BaseUrl is skipped for a non-custom template.
    assert_eq!(w.step, ProviderWizardStep::ApiKey);
    assert!(w.advance());
    assert_eq!(w.step, ProviderWizardStep::Model);
    assert!(w.advance());
    assert_eq!(w.step, ProviderWizardStep::Confirm);
    // No step past Confirm.
    assert!(!w.advance());
}

#[test]
fn advance_visits_base_url_for_custom_template() {
    let mut w = ProviderWizardState::new(vec![ProviderTemplate::custom()]);
    assert!(w.is_custom());
    assert!(w.advance());
    assert_eq!(w.step, ProviderWizardStep::Name);
    assert!(w.advance());
    assert_eq!(w.step, ProviderWizardStep::BaseUrl);
    assert!(w.advance());
    assert_eq!(w.step, ProviderWizardStep::ApiKey);
}

#[test]
fn back_navigates_and_stops_at_template() {
    let mut w = ProviderWizardState::new(vec![catalog_tpl(), ProviderTemplate::custom()]);
    w.advance();
    w.advance();
    assert_eq!(w.step, ProviderWizardStep::ApiKey);
    assert!(w.back());
    assert_eq!(w.step, ProviderWizardStep::Name);
    assert!(w.back());
    assert_eq!(w.step, ProviderWizardStep::Template);
    // Already at the first step.
    assert!(!w.back());
}

#[test]
fn build_partial_catalog_uses_env_key_and_template_base_url() {
    let w = ProviderWizardState::new(vec![catalog_tpl(), ProviderTemplate::custom()]);
    let v = serde_json::to_value(build_partial(&w)).unwrap();
    assert_eq!(v["api"], "openai_compat");
    assert_eq!(v["base_url"], "https://api.deepseek.com/v1");
    assert_eq!(v["env_key"], "DEEPSEEK_API_KEY");
    assert_eq!(v["wire_api"], "chat");
    // No key / model entered → those fields are omitted.
    assert!(v.get("api_key").is_none());
    assert!(v.get("models").is_none());
}

#[test]
fn build_partial_custom_persists_key_and_model() {
    let mut w = ProviderWizardState::new(vec![ProviderTemplate::custom()]);
    w.base_url = WizardTextField::seeded("https://api.example.com/v1");
    w.api_key = WizardTextField::seeded("sk-secret-123");
    w.model_id = WizardTextField::seeded("my-model");
    let v = serde_json::to_value(build_partial(&w)).unwrap();
    assert_eq!(v["api"], "openai_compat");
    assert_eq!(v["base_url"], "https://api.example.com/v1");
    // RedactedSecret is `#[serde(transparent)]` → the real key is persisted.
    assert_eq!(v["api_key"], "sk-secret-123");
    assert!(v["models"].get("my-model").is_some());
}

#[test]
fn resolved_name_falls_back_to_template_default() {
    let w = ProviderWizardState::new(vec![catalog_tpl()]);
    assert_eq!(w.resolved_name(), "deepseek-openai");
}

#[test]
fn validate_base_url_rejects_non_http() {
    let mut w = ProviderWizardState::new(vec![ProviderTemplate::custom()]);
    w.step = ProviderWizardStep::BaseUrl;
    w.base_url = WizardTextField::seeded("api.example.com");
    assert!(validate_step(&w).is_some());
    w.base_url = WizardTextField::seeded("https://api.example.com/v1");
    assert!(validate_step(&w).is_none());
}
