pub mod client_options;
pub mod model_override;

pub use client_options::HeaderValue;
pub use client_options::PartialProviderClientOptions;
pub use client_options::ProviderClientOptions;
pub use model_override::PartialProviderModelOverride;
pub use model_override::ProviderModelOverride;

use crate::env;
use crate::error::ConfigError;
use crate::error::ConfigField;
use crate::secret::RedactedSecret;
use coco_types::OAuthFlowId;
use coco_types::ProviderApi;
use coco_types::WireApi;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;

/// How a provider instance authenticates.
///
/// `ApiKey` (default) reads `env_key` / config `api_key` — the historical
/// behavior. `OAuth` routes through `coco-provider-auth`: credentials are minted
/// by `coco login`, persisted provider-scoped, and auto-refreshed; the owning
/// `vercel-ai-<provider>` crate's `*Auth` wire mode consumes them. `env_key` /
/// `api_key` are ignored under `OAuth`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderAuth {
    #[default]
    ApiKey,
    OAuth {
        flow: OAuthFlowId,
    },
}

/// Wire format for `~/.coco/providers.json` and the per-user
/// `settings.providers.<name>` overlay. Every field is `Option` so an
/// overlay never silently coerces a non-set field to a serde default.
///
/// # Identity invariant
///
/// There is intentionally **no `name` field** here — the provider
/// identifier is the parent map key in
/// `BTreeMap<String, PartialProviderConfig>`.
/// `serde(deny_unknown_fields)` rejects user-written `name` at parse
/// time; `from_partial(map_key, partial)` writes
/// `resolved.name = map_key.to_string()` exactly once.
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct PartialProviderConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api: Option<ProviderApi>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env_key: Option<String>,
    /// Fallback API key from config file. Prefer `env_key` env var.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<RedactedSecret>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wire_api: Option<WireApi>,
    /// Authentication mode. Defaults to `ApiKey`. `OAuth { flow }` selects a
    /// subscription login managed by `coco-provider-auth`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<ProviderAuth>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_options: Option<PartialProviderClientOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<BTreeMap<String, PartialProviderModelOverride>>,
    /// Provider-instance behavior knobs. Opaque at the config layer —
    /// each `vercel-ai-<provider>` crate parses its own slice via a
    /// `parse_provider_options(&BTreeMap<String, Value>)` helper. Keys
    /// are flat (no nesting) and merge key-by-key across settings sources
    /// (later source wins per key, identical semantics to
    /// `client_options.headers`).
    ///
    /// Example shape for Anthropic:
    /// `{"experimental_betas": false, "disable_interleaved_thinking": true}`.
    /// Unknown keys are ignored at the config layer; the provider crate
    /// rejects unknown keys at parse time.
    ///
    /// Distinct from `client_options` (transport / auth / URL). This
    /// field is for request-policy / behavior knobs that don't fit the
    /// transport abstraction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<BTreeMap<String, Value>>,
}

impl fmt::Debug for PartialProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PartialProviderConfig")
            .field("api", &self.api)
            .field("env_key", &self.env_key)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("base_url", &self.base_url)
            .field("timeout_secs", &self.timeout_secs)
            .field("wire_api", &self.wire_api)
            .field("auth", &self.auth)
            .field("client_options", &self.client_options)
            .field("models", &self.models)
            .field("provider_options", &self.provider_options)
            .finish()
    }
}

const DEFAULT_TIMEOUT_SECS: i64 = 600;

/// Resolved per-provider configuration. `name` is set in exactly one
/// place — `from_partial` — from the parent map key. There is no path
/// for a divergence between the map key and `name`.
///
/// `Debug` is implemented manually so `api_key` cannot leak through
/// `tracing::error!("{cfg:?}")`, snafu cause chains, or panic
/// formatters.
#[derive(Clone, Serialize)]
pub struct ProviderConfig {
    /// Identity = parent map key, written exactly once in `from_partial`.
    pub name: String,
    pub api: ProviderApi,
    pub env_key: String,
    /// Fallback API key from config file. `RedactedSecret` redacts
    /// `Debug`/`Display`; `.expose()` is the single audit point.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<RedactedSecret>,
    pub base_url: String,
    pub timeout_secs: i64,
    pub wire_api: WireApi,
    /// Authentication mode. `ApiKey` (default) uses `env_key` / `api_key`;
    /// `OAuth { flow }` uses `coco-provider-auth`-managed subscription creds.
    pub auth: ProviderAuth,
    pub client_options: ProviderClientOptions,
    /// Per-(provider, model) entries — `BTreeMap` so on-disk
    /// serialisation is byte-stable.
    pub models: BTreeMap<String, ProviderModelOverride>,
    /// Provider-instance behavior knobs (opaque at this layer). The
    /// owning `vercel-ai-<provider>` crate parses its own slice via a
    /// `parse_provider_options` helper. `BTreeMap` keeps the map
    /// byte-stable for fingerprint/digest hashing.
    pub provider_options: BTreeMap<String, Value>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            api: ProviderApi::Anthropic,
            env_key: String::new(),
            api_key: None,
            base_url: String::new(),
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            wire_api: WireApi::Chat,
            auth: ProviderAuth::default(),
            client_options: ProviderClientOptions::default(),
            models: BTreeMap::new(),
            provider_options: BTreeMap::new(),
        }
    }
}

impl fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("name", &self.name)
            .field("api", &self.api)
            .field("env_key", &self.env_key)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("base_url", &self.base_url)
            .field("timeout_secs", &self.timeout_secs)
            .field("wire_api", &self.wire_api)
            .field("client_options", &self.client_options)
            .field("models", &self.models)
            .field("provider_options", &self.provider_options)
            .finish()
    }
}

impl ProviderConfig {
    /// Build a fully-resolved entry from a partial overlay. `name` is
    /// taken from `map_key` — the partial cannot supply one (rejected
    /// at parse time by `deny_unknown_fields`). Required fields
    /// (`api`, `env_key`, `base_url`) must be `Some(_)` or this
    /// returns `ConfigError::IncompleteProviderEntry`.
    pub fn from_partial(
        map_key: &str,
        partial: &PartialProviderConfig,
    ) -> Result<Self, ConfigError> {
        let api = partial.api.ok_or(ConfigError::IncompleteProviderEntry {
            name: map_key.to_string(),
            field: ConfigField::Api,
        })?;
        let auth = partial.auth.clone().unwrap_or_default();
        // OAuth providers authenticate via `coco-provider-auth`, not an env var
        // or config `api_key`, so `env_key` is irrelevant — default it to empty
        // rather than rejecting the (otherwise valid) entry. ApiKey providers
        // still require it.
        let env_key = match partial.env_key.clone() {
            Some(k) => k,
            None if matches!(auth, ProviderAuth::OAuth { .. }) => String::new(),
            None => {
                return Err(ConfigError::IncompleteProviderEntry {
                    name: map_key.to_string(),
                    field: ConfigField::EnvKey,
                });
            }
        };
        let base_url = partial
            .base_url
            .clone()
            .ok_or(ConfigError::IncompleteProviderEntry {
                name: map_key.to_string(),
                field: ConfigField::BaseUrl,
            })?;

        // A negative `timeout_secs` is unambiguously a typo — neither
        // "default" (use `None`) nor "disabled" (use `0`). Rejecting at
        // the boundary turns a silent unbounded-request bug into a
        // startup error.
        if let Some(t) = partial.timeout_secs
            && t < 0
        {
            return Err(ConfigError::InvalidTimeoutSecs {
                name: map_key.to_string(),
                value: t,
            });
        }

        let client_options = partial
            .client_options
            .as_ref()
            .map(ProviderClientOptions::from_partial)
            .unwrap_or_default();

        let models = partial
            .models
            .as_ref()
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), ProviderModelOverride::from_partial(v.clone())))
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();

        let provider_options = partial.provider_options.clone().unwrap_or_default();

        Ok(Self {
            name: map_key.to_string(),
            api,
            env_key,
            api_key: partial.api_key.clone(),
            base_url,
            timeout_secs: partial.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS),
            wire_api: partial.wire_api.unwrap_or(WireApi::Chat),
            auth,
            client_options,
            models,
            provider_options,
        })
    }

    /// Layer a partial overlay over `self`. `api` is taken from the
    /// overlay only when `Some` — never silently coerced to a serde
    /// default. `models` merges entry-by-entry; nested
    /// `client_options` merges field-by-field, and `client_options.headers`
    /// itself merges key-by-key (overlay wins per key).
    ///
    /// Returns [`ConfigError::InvalidTimeoutSecs`] when the overlay
    /// supplies a negative `timeout_secs`. Same boundary check as
    /// [`Self::from_partial`] — neither path should silently accept a
    /// typo'd negative value.
    pub fn merge_partial(&mut self, overlay: &PartialProviderConfig) -> Result<(), ConfigError> {
        if let Some(api) = overlay.api {
            self.api = api;
        }
        if let Some(env_key) = &overlay.env_key {
            self.env_key.clone_from(env_key);
        }
        if let Some(api_key) = &overlay.api_key {
            self.api_key = Some(api_key.clone());
        }
        if let Some(base_url) = &overlay.base_url {
            self.base_url.clone_from(base_url);
        }
        if let Some(timeout) = overlay.timeout_secs {
            if timeout < 0 {
                return Err(ConfigError::InvalidTimeoutSecs {
                    name: self.name.clone(),
                    value: timeout,
                });
            }
            self.timeout_secs = timeout;
        }
        if let Some(wire_api) = overlay.wire_api {
            self.wire_api = wire_api;
        }
        if let Some(auth) = &overlay.auth {
            self.auth = auth.clone();
        }
        if let Some(client_opts) = &overlay.client_options {
            self.client_options.merge_partial(client_opts);
        }
        if let Some(models) = &overlay.models {
            for (k, v) in models {
                self.models
                    .insert(k.clone(), ProviderModelOverride::from_partial(v.clone()));
            }
        }
        // `provider_options` merges key-by-key — overlay wins per key.
        // Same shape semantics as `client_options.headers`. A key set
        // to `Value::Null` removes the field; that's the only way
        // a downstream overlay can opt-out of a key set higher up.
        if let Some(opts) = &overlay.provider_options {
            for (k, v) in opts {
                if v.is_null() {
                    self.provider_options.remove(k);
                } else {
                    self.provider_options.insert(k.clone(), v.clone());
                }
            }
        }
        Ok(())
    }

    /// Resolve API key for this provider.
    /// Priority: env var > config file `api_key` > none.
    ///
    /// Returns `Option<String>` (not `RedactedSecret`) because the
    /// caller (`model_factory::build_*`) hands the raw key to vercel-ai
    /// `*ProviderSettings.api_key`. `.expose()` here is one of the
    /// two grep-able audit points.
    pub fn resolve_api_key(&self) -> Option<String> {
        env::env_opt(&self.env_key)
            .or_else(|| self.api_key.as_ref().map(|s| s.expose().to_string()))
    }
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
