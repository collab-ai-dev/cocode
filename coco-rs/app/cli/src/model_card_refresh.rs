use std::sync::Once;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use coco_config::RuntimeConfig;
use coco_types::Feature;
use tracing::info;
use tracing::warn;

const OPENROUTER_MODELS_URL: &str = "https://openrouter.ai/api/v1/models";
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_RESPONSE_BYTES: usize = 5 * 1024 * 1024;

static START_REFRESH: Once = Once::new();

/// Fire-and-forget dynamic model-card refresh.
///
/// Startup must not wait on network I/O. Callers invoke this after resolving
/// `RuntimeConfig`; when the feature is enabled we spawn one background task
/// per process and keep using the bundled catalog until the fetch completes.
pub fn spawn_if_enabled(runtime_config: &RuntimeConfig) {
    if !runtime_config.features.enabled(Feature::DynamicModelCard) {
        return;
    }

    START_REFRESH.call_once(|| {
        tokio::spawn(async {
            match fetch_openrouter_models().await {
                Ok(json) => match coco_model_card::install_openrouter_snapshot(&json) {
                    Ok(()) => info!("dynamic model-card catalog refreshed"),
                    Err(err) => {
                        warn!(error = %err, "failed to install dynamic model-card catalog");
                    }
                },
                Err(err) => {
                    warn!(error = %err, "failed to fetch dynamic model-card catalog");
                }
            }
        });
    });
}

async fn fetch_openrouter_models() -> Result<String> {
    let mut response = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .user_agent(concat!("coco/", env!("CARGO_PKG_VERSION")))
        .build()?
        .get(OPENROUTER_MODELS_URL)
        .send()
        .await?
        .error_for_status()?;

    if let Some(len) = response.content_length()
        && len > MAX_RESPONSE_BYTES as u64
    {
        anyhow::bail!("OpenRouter model catalog is too large: {len} bytes");
    }

    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            anyhow::bail!("OpenRouter model catalog exceeded {MAX_RESPONSE_BYTES} bytes");
        }
        body.extend_from_slice(&chunk);
    }

    String::from_utf8(body).context("OpenRouter model catalog was not valid UTF-8")
}
