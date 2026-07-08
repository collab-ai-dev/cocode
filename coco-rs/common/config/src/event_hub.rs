use serde::Deserialize;
use serde::Serialize;

use crate::ConfigError;
use crate::EnvKey;
use crate::EnvSnapshot;
use crate::RuntimeOverrides;
use crate::settings::Settings;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EventHubConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl EventHubConfig {
    pub fn resolve(
        settings: &Settings,
        env: &EnvSnapshot,
        overrides: &RuntimeOverrides,
    ) -> crate::Result<Self> {
        let url = overrides
            .event_hub_url_override
            .clone()
            .or_else(|| env.get_string(EnvKey::CocoEventHubUrl))
            .or_else(|| settings.event_hub_url.clone())
            .and_then(normalize_url)
            .map(validate_ws_url)
            .transpose()?;
        Ok(Self { url })
    }
}

fn normalize_url(raw: String) -> Option<String> {
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn validate_ws_url(url: String) -> crate::Result<String> {
    if url.starts_with("ws://") || url.starts_with("wss://") {
        return Ok(url);
    }
    Err(ConfigError::invalid_config(format!(
        "event_hub_url must start with ws:// or wss:// (got {url:?})"
    )))
}
