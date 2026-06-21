use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::RwLock;

use crate::resolver::aliases_for_openrouter_model;
use crate::resolver::family_for_model;
use crate::resolver::lookup_key_tiers;
use crate::resolver::normalize_id;
use crate::resolver::provider_from_id;
use crate::resolver::strip_provider;
use crate::schema::KnowledgeCutoff;
use crate::schema::ModelCard;
use crate::schema::ModelCardError;
use crate::schema::OpenRouterModel;
use crate::schema::OpenRouterModelsResponse;
use crate::schema::Pricing;

const BUNDLED_OPENROUTER_MODELS_JSON: &str = include_str!("../data/openrouter-models.json");

#[derive(Debug, Clone, PartialEq)]
pub enum LookupResult {
    Found(Arc<ModelCard>),
    NotFound,
    Ambiguous(Vec<String>),
}

#[derive(Debug, Clone)]
pub struct ModelCardCatalog {
    cards: Vec<Arc<ModelCard>>,
    exact_index: BTreeMap<String, Vec<usize>>,
    stripped_index: BTreeMap<String, Vec<usize>>,
}

impl ModelCardCatalog {
    pub fn from_openrouter_json(json: &str) -> Result<Self, ModelCardError> {
        let response: OpenRouterModelsResponse = serde_json::from_str(json)?;
        if response.data.is_empty() {
            return Err(ModelCardError::EmptyCatalog);
        }
        Ok(Self::from_openrouter_models(response.data))
    }

    pub fn lookup(&self, model_id: &str) -> LookupResult {
        self.lookup_with_provider(None, model_id)
    }

    pub fn lookup_with_provider(&self, provider: Option<&str>, model_id: &str) -> LookupResult {
        if model_id.trim().is_empty() {
            return LookupResult::NotFound;
        }

        for (tier_idx, tier) in lookup_key_tiers(provider, model_id).into_iter().enumerate() {
            let index = if tier_idx < 2 {
                &self.exact_index
            } else {
                &self.stripped_index
            };
            let mut matched = BTreeSet::new();
            for key in tier {
                if let Some(indices) = index.get(&key) {
                    matched.extend(indices.iter().copied());
                }
            }
            match matched.len() {
                0 => continue,
                1 => {
                    if let Some(idx) = matched.into_iter().next() {
                        return LookupResult::Found(self.cards[idx].clone());
                    }
                    return LookupResult::NotFound;
                }
                _ => {
                    return LookupResult::Ambiguous(
                        matched
                            .into_iter()
                            .map(|idx| self.cards[idx].canonical_id.clone())
                            .collect(),
                    );
                }
            }
        }

        LookupResult::NotFound
    }

    pub fn len(&self) -> usize {
        self.cards.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }

    fn from_openrouter_models(models: Vec<OpenRouterModel>) -> Self {
        let mut cards = Vec::new();
        for model in models {
            let provider = provider_from_id(&model.id);
            let canonical_slug = model.canonical_slug.clone();
            let family =
                family_for_model(provider.as_deref(), &model.id, canonical_slug.as_deref());
            let aliases = aliases_for_openrouter_model(&model);
            let pricing = model.pricing.as_ref().and_then(Pricing::from_openrouter);
            let knowledge_cutoff = curated_knowledge_cutoff(&model.id)
                .or_else(|| canonical_slug.as_deref().and_then(curated_knowledge_cutoff))
                .or_else(|| {
                    model
                        .knowledge_cutoff
                        .as_deref()
                        .and_then(KnowledgeCutoff::from_raw)
                });
            let vendor_context_window = model
                .top_provider
                .as_ref()
                .and_then(|p| p.context_length)
                .or(model.context_length);

            cards.push(Arc::new(ModelCard {
                canonical_id: model.id,
                aliases,
                family,
                knowledge_cutoff,
                pricing,
                vendor_context_window,
                display_name: model.name,
            }));
        }
        Self::new(cards)
    }

    fn new(cards: Vec<Arc<ModelCard>>) -> Self {
        let mut exact_index: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        let mut stripped_index: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (idx, card) in cards.iter().enumerate() {
            let provider = provider_from_id(&card.canonical_id);
            index_model_keys(
                provider.as_deref(),
                &card.canonical_id,
                idx,
                &mut exact_index,
                &mut stripped_index,
            );
            for alias in &card.aliases {
                index_model_keys(
                    provider.as_deref(),
                    alias,
                    idx,
                    &mut exact_index,
                    &mut stripped_index,
                );
            }
        }

        Self {
            cards,
            exact_index,
            stripped_index,
        }
    }
}

fn index_model_keys(
    provider: Option<&str>,
    model_id: &str,
    idx: usize,
    exact_index: &mut BTreeMap<String, Vec<usize>>,
    stripped_index: &mut BTreeMap<String, Vec<usize>>,
) {
    for (tier_idx, tier) in lookup_key_tiers(provider, model_id).into_iter().enumerate() {
        let index = if tier_idx < 2 {
            &mut *exact_index
        } else {
            &mut *stripped_index
        };
        for key in tier {
            index.entry(key).or_default().push(idx);
        }
    }
}

fn curated_knowledge_cutoff(model_id: &str) -> Option<KnowledgeCutoff> {
    let normalized = normalize_id(strip_provider(model_id.trim()));

    let raw = match normalized.as_str() {
        "claude-opus-4-7" | "claude-opus-4-7-fast" | "claude-4-7-opus" | "claude-4-7-opus-fast" => {
            "2026-01-31"
        }
        "claude-sonnet-4-6" | "claude-4-6-sonnet" => "2025-08-31",
        "claude-haiku-4-5" | "claude-4-5-haiku" => "2025-02-28",
        "gpt-5-4" | "gpt-5-4-pro" => "2025-08-31",
        "gpt-5-5" | "gpt-5-5-pro" => "2025-12-01",
        _ => return None,
    };
    KnowledgeCutoff::from_raw(raw)
}

/// Exact/normalized lookup against the current in-memory catalog.
pub fn lookup(model_id: &str) -> Option<Arc<ModelCard>> {
    match current_catalog().lookup(model_id) {
        LookupResult::Found(card) => Some(card),
        LookupResult::NotFound | LookupResult::Ambiguous(_) => None,
    }
}

/// Provider-aware lookup against the current in-memory catalog.
pub fn lookup_with_provider(provider: Option<&str>, model_id: &str) -> LookupResult {
    current_catalog().lookup_with_provider(provider, model_id)
}

/// Convenience for the env-block call site.
pub fn knowledge_cutoff(model_id: &str) -> Option<String> {
    lookup(model_id)?
        .knowledge_cutoff
        .as_ref()
        .map(|c| c.display.clone())
}

/// Bare model name for display in the `<env>` block ("You are powered by
/// the model …"). Strips any `provider/` prefix (mirroring the catalog's
/// stripped-index lookup) so a `provider/model_id` selection renders the
/// same bare id the rest of the system shows, while **preserving the
/// configured id spelling** — the agent is told the model it is actually
/// running, not a re-canonicalized slug. Empty input stays empty so the
/// env line is omitted.
pub fn display_model_name(raw: &str) -> String {
    strip_provider(raw).to_string()
}

/// Convenience for cost-tracking and display call sites.
pub fn pricing(provider: Option<&str>, model_id: &str) -> Option<Pricing> {
    match current_catalog().lookup_with_provider(provider, model_id) {
        LookupResult::Found(card) => card.pricing.clone(),
        LookupResult::NotFound | LookupResult::Ambiguous(_) => None,
    }
}

/// Atomically replaces the current in-memory catalog with a parsed
/// OpenRouter snapshot. Intended for startup background refresh.
///
/// Pricing in the supplied JSON is taken verbatim — OpenRouter is the
/// sole source of truth, and there is no vendor-side override layer.
/// Callers must trust the snapshot's numbers; install only sources you
/// would accept for cost reporting.
pub fn install_openrouter_snapshot(json: &str) -> Result<(), ModelCardError> {
    let catalog = Arc::new(ModelCardCatalog::from_openrouter_json(json)?);
    let lock = catalog_cell();
    let mut guard = lock
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *guard = catalog;
    Ok(())
}

/// Parse the bundled snapshot. Exposed for tests and diagnostics.
pub fn bundled_catalog() -> Result<ModelCardCatalog, ModelCardError> {
    ModelCardCatalog::from_openrouter_json(BUNDLED_OPENROUTER_MODELS_JSON)
}

fn current_catalog() -> Arc<ModelCardCatalog> {
    catalog_cell()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

fn catalog_cell() -> &'static RwLock<Arc<ModelCardCatalog>> {
    static CELL: OnceLock<RwLock<Arc<ModelCardCatalog>>> = OnceLock::new();
    CELL.get_or_init(|| {
        let catalog = bundled_catalog().unwrap_or_else(|err| {
            panic!("bundled OpenRouter model-card snapshot must parse: {err}")
        });
        RwLock::new(Arc::new(catalog))
    })
}
