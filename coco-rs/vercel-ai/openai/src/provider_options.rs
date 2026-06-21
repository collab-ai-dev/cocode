//! Adapter-owned parser for the per-provider-instance behavior knobs carried
//! in `ProviderConfig.provider_options`.
//!
//! Schema is owned by **this crate**, not by `coco-config`. The infrastructure
//! layer transports an opaque `BTreeMap<String, Value>`; we deserialize it here
//! into a typed struct with `deny_unknown_fields` so a typo
//! (`reasoning_stor`) fails at startup rather than silently shipping the
//! default. Mirrors `vercel-ai-anthropic::parse_provider_options`.
//!
//! Settings example (`~/.coco/provider.json`):
//!
//! ```json
//! {
//!   "openai": {
//!     "api": "openai",
//!     "base_url": "https://api.openai.com/v1",
//!     "env_key": "OPENAI_API_KEY",
//!     "provider_options": {
//!       "reasoning_store": "stateless"
//!     }
//!   }
//! }
//! ```
//!
//! Unset fields fall through to the typed defaults (`reasoning_store =
//! "server"`).

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

use crate::openai_config::ResponsesStorePolicy;

/// Resolved OpenAI per-provider behavior knobs. Concrete (no `Option`) so
/// callers have a fully-determined view by the time `OpenAIProviderSettings`
/// is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OpenAIProviderOptionsConfig {
    /// Policy for the Responses `store` field on reasoning models. Default
    /// `ServerDefault` (omit `store` — server-side reasoning state).
    pub reasoning_store: ResponsesStorePolicy,
}

/// Wire shape — what the JSON in `provider_options` looks like. Every field is
/// optional so a partial map is valid (missing → the typed default).
/// `deny_unknown_fields` surfaces typos at startup.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct OpenAIProviderOptionsRaw {
    reasoning_store: Option<ResponsesStorePolicy>,
}

/// Errors produced by [`parse_provider_options`]. Typed so `services/inference`
/// can attach structured context (provider name, settings source) on surface.
#[derive(Debug, thiserror::Error)]
pub enum ProviderOptionsError {
    /// JSON shape didn't deserialize into the typed schema. Carries the
    /// underlying `serde_json::Error` for the field path / line / column.
    #[error("invalid openai provider_options: {0}")]
    Invalid(#[from] serde_json::Error),
}

/// Parse the opaque `BTreeMap<String, Value>` from
/// `ProviderConfig.provider_options` into a typed config. Empty map → all
/// defaults. Routes through `serde_json::Value` so `deny_unknown_fields`
/// enforcement is automatic.
pub fn parse_provider_options(
    options: &BTreeMap<String, Value>,
) -> Result<OpenAIProviderOptionsConfig, ProviderOptionsError> {
    if options.is_empty() {
        return Ok(OpenAIProviderOptionsConfig::default());
    }
    let value = Value::Object(
        options
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    );
    let raw: OpenAIProviderOptionsRaw = serde_json::from_value(value)?;
    Ok(OpenAIProviderOptionsConfig {
        reasoning_store: raw.reasoning_store.unwrap_or_default(),
    })
}

#[cfg(test)]
#[path = "provider_options.test.rs"]
mod tests;
