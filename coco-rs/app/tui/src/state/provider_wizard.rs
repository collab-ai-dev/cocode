//! `/provider` add-provider wizard state.
//!
//! A linear, mostly select-driven step machine modeled on the `/agents`
//! create wizard ([`super::agents_dialog::CreateWizardState`]): pick a
//! provider template from the builtin catalog, then supply only the values a
//! template can't prefill (the secret, and for a custom provider its base
//! URL). Stage 2 (choosing a model) is delegated to the existing `/model`
//! picker after the provider is written, mirroring opencode's flow.

use coco_types::ProviderApi;
use coco_types::WireApi;

use super::agents_dialog::WizardTextField;

/// A selectable provider preset. Catalog entries come from
/// `coco_config::builtin::builtin_provider_partials()` with `api` / `base_url`
/// / `wire_api` prefilled; the trailing synthetic entry (`is_custom`) prompts
/// for a base URL instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderTemplate {
    /// Catalog key, used as the default instance name (e.g. `deepseek-openai`).
    pub name: String,
    pub api: ProviderApi,
    pub base_url: String,
    pub wire_api: WireApi,
    /// Env var the provider reads its key from (e.g. `OPENAI_API_KEY`).
    pub env_key: String,
    /// The synthetic "Custom (OpenAI-compatible)" row — prompts for `base_url`.
    pub is_custom: bool,
}

impl ProviderTemplate {
    /// The synthetic custom entry appended after the catalog rows.
    pub fn custom() -> Self {
        Self {
            name: "custom".to_string(),
            api: ProviderApi::OpenaiCompat,
            base_url: String::new(),
            wire_api: WireApi::Chat,
            env_key: String::new(),
            is_custom: true,
        }
    }
}

/// Linear wizard steps. `BaseUrl` is visited only for a custom template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderWizardStep {
    Template,
    Name,
    BaseUrl,
    ApiKey,
    Model,
    Confirm,
}

impl ProviderWizardStep {
    const ORDER: [Self; 6] = [
        Self::Template,
        Self::Name,
        Self::BaseUrl,
        Self::ApiKey,
        Self::Model,
        Self::Confirm,
    ];

    fn index(self) -> usize {
        Self::ORDER
            .iter()
            .position(|s| *s == self)
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
pub struct ProviderWizardState {
    pub step: ProviderWizardStep,
    /// Catalog templates + a trailing custom entry.
    pub templates: Vec<ProviderTemplate>,
    /// Highlighted template on the `Template` step.
    pub template_idx: usize,
    /// Instance name, defaulted from the selected template.
    pub name: WizardTextField,
    /// Base URL — edited only for a custom template.
    pub base_url: WizardTextField,
    /// API key. Empty ⇒ rely on the provider's env var.
    pub api_key: WizardTextField,
    /// Optional model id to register with the new provider.
    pub model_id: WizardTextField,
    /// Inline validation message under the active field.
    pub error: Option<String>,
    /// Settings path written on the `Confirm` step; drives the success line.
    pub saved_path: Option<String>,
}

impl ProviderWizardState {
    pub fn new(mut templates: Vec<ProviderTemplate>) -> Self {
        if templates.is_empty() {
            templates.push(ProviderTemplate::custom());
        }
        let name = WizardTextField::seeded(&templates[0].name);
        Self {
            step: ProviderWizardStep::Template,
            templates,
            template_idx: 0,
            name,
            base_url: WizardTextField::new(),
            api_key: WizardTextField::new(),
            model_id: WizardTextField::new(),
            error: None,
            saved_path: None,
        }
    }

    pub fn selected_template(&self) -> &ProviderTemplate {
        &self.templates[self.template_idx.min(self.templates.len() - 1)]
    }

    pub fn is_custom(&self) -> bool {
        self.selected_template().is_custom
    }

    /// The text field edited on the active step, if any.
    pub fn active_field(&self) -> Option<&WizardTextField> {
        match self.step {
            ProviderWizardStep::Name => Some(&self.name),
            ProviderWizardStep::BaseUrl => Some(&self.base_url),
            ProviderWizardStep::ApiKey => Some(&self.api_key),
            ProviderWizardStep::Model => Some(&self.model_id),
            ProviderWizardStep::Template | ProviderWizardStep::Confirm => None,
        }
    }

    pub fn active_field_mut(&mut self) -> Option<&mut WizardTextField> {
        match self.step {
            ProviderWizardStep::Name => Some(&mut self.name),
            ProviderWizardStep::BaseUrl => Some(&mut self.base_url),
            ProviderWizardStep::ApiKey => Some(&mut self.api_key),
            ProviderWizardStep::Model => Some(&mut self.model_id),
            ProviderWizardStep::Template | ProviderWizardStep::Confirm => None,
        }
    }

    /// Move the template highlight by `delta`, wrapping, and re-seed the name.
    pub fn cycle_template(&mut self, delta: i32) {
        let n = self.templates.len() as i32;
        if n == 0 {
            return;
        }
        self.template_idx = (self.template_idx as i32 + delta).rem_euclid(n) as usize;
        self.name = WizardTextField::seeded(&self.selected_template().name);
        self.error = None;
    }

    /// Advance to the next step, skipping `BaseUrl` for non-custom templates.
    /// Returns `false` when already on `Confirm`.
    pub fn advance(&mut self) -> bool {
        let mut idx = self.step.index();
        loop {
            if idx + 1 >= ProviderWizardStep::ORDER.len() {
                return false;
            }
            idx += 1;
            let next = ProviderWizardStep::ORDER[idx];
            if next == ProviderWizardStep::BaseUrl && !self.is_custom() {
                continue;
            }
            self.step = next;
            self.error = None;
            return true;
        }
    }

    /// Step back one step (skipping `BaseUrl` for non-custom templates).
    /// Returns `false` when already on the first step.
    pub fn back(&mut self) -> bool {
        let mut idx = self.step.index();
        loop {
            if idx == 0 {
                return false;
            }
            idx -= 1;
            let prev = ProviderWizardStep::ORDER[idx];
            if prev == ProviderWizardStep::BaseUrl && !self.is_custom() {
                continue;
            }
            self.step = prev;
            self.error = None;
            return true;
        }
    }

    /// Trimmed instance name, falling back to the template's default.
    pub fn resolved_name(&self) -> String {
        let trimmed = self.name.text.trim();
        if trimmed.is_empty() {
            self.selected_template().name.clone()
        } else {
            trimmed.to_string()
        }
    }

    /// Effective base URL: the edited value for custom, else the template's.
    pub fn resolved_base_url(&self) -> String {
        if self.is_custom() {
            self.base_url.text.trim().to_string()
        } else {
            self.selected_template().base_url.clone()
        }
    }
}
