//! Post-login `/models` discovery for OpenAI-family providers.
//!
//! Fire-and-forget: after a successful `/login`, a time-bounded background task
//! asks the provider for its live model list and merges any NEW ids onto the
//! static catalog, then pushes `ModelCatalogRefreshed` so the `/model` picker
//! shows subscription-only models without a restart. Mirrors
//! `model_card_refresh` — log-and-degrade, never blocks the TUI or the login
//! transcript response.

use std::collections::HashSet;
use std::time::Duration;

use coco_types::CoreEvent;
use coco_types::ModelCatalogInfo;
use coco_types::ProviderApi;
use coco_types::TuiOnlyEvent;
use tokio::sync::mpsc;

use crate::session_runtime::SessionHandle;
use crate::session_runtime::SessionRuntime;

/// Hard cap so a hung `/models` request can never wedge the refresh task.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);
/// HTTP client timeout for the one-shot discovery call.
const DISCOVERY_HTTP_TIMEOUT_SECS: i64 = 20;

/// Spawn a background `/models` refresh for the just-logged-in `instance`.
/// A no-op for non-OpenAI providers (the task exits before emitting anything).
pub fn spawn_after_login(
    session: SessionHandle,
    instance: String,
    event_tx: mpsc::Sender<CoreEvent>,
    base_catalog: Vec<ModelCatalogInfo>,
) {
    let runtime = session.runtime().clone();
    tokio::spawn(async move {
        let Some(discovered) = discover(&runtime, &instance).await else {
            return;
        };
        let merged = merge(base_catalog, &instance, discovered);
        let _ = event_tx
            .send(CoreEvent::Tui(TuiOnlyEvent::ModelCatalogRefreshed {
                entries: merged,
            }))
            .await;
    });
}

/// Query the provider's live model list, or `None` when the provider is not an
/// OpenAI-family instance, isn't configured, or the call fails / times out.
async fn discover(runtime: &SessionRuntime, instance: &str) -> Option<Vec<(String, Option<i64>)>> {
    let cfg = runtime.runtime_config.providers.get(instance)?;
    // The `/models` listing is OpenAI-shaped (codex backend + platform). Other
    // APIs (Anthropic, Gemini, generic openai-compat) are out of scope.
    if cfg.api != ProviderApi::Openai {
        return None;
    }
    let resolver = crate::provider_login::shared_resolver();
    let fut = coco_inference::model_factory::discover_openai_models(
        cfg,
        Some(&resolver),
        DISCOVERY_HTTP_TIMEOUT_SECS,
    );
    match tokio::time::timeout(DISCOVERY_TIMEOUT, fut).await {
        Ok(Ok(models)) => Some(models),
        Ok(Err(e)) => {
            tracing::debug!(provider = instance, error = %e, "model discovery failed");
            None
        }
        Err(_) => {
            tracing::debug!(provider = instance, "model discovery timed out");
            None
        }
    }
}

/// Merge discovered ids onto the static catalog: keep the (richer) static entry
/// for ids already present, and append a minimal entry for each genuinely new
/// discovered id. The static base is never dropped, so the merge is additive.
fn merge(
    mut base: Vec<ModelCatalogInfo>,
    instance: &str,
    discovered: Vec<(String, Option<i64>)>,
) -> Vec<ModelCatalogInfo> {
    let known: HashSet<String> = base
        .iter()
        .filter(|e| e.provider == instance)
        .map(|e| e.model_id.clone())
        .collect();
    // Reuse the provider's display label from any existing static row so the
    // new rows group under the same section header.
    let provider_display = base
        .iter()
        .find(|e| e.provider == instance)
        .map(|e| e.provider_display.clone())
        .unwrap_or_else(|| instance.to_string());
    for (id, context_window) in discovered {
        if known.contains(&id) {
            continue;
        }
        base.push(ModelCatalogInfo {
            provider: instance.to_string(),
            provider_display: provider_display.clone(),
            model_id: id.clone(),
            display_name: id,
            context_window,
            supported_efforts: Vec::new(),
            default_effort: None,
        });
    }
    base
}

#[cfg(test)]
#[path = "openai_model_refresh.test.rs"]
mod tests;
