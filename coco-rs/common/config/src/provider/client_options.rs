//! Typed `ProviderClientOptions` — replaces the loose `HashMap<String, JSONValue>`
//! shape that earlier drafts used.
//!
//! Each field is opt-in; missing fields mean "SDK default". `serde_json`
//! reports field-level errors with JSON pointers, `deny_unknown_fields`
//! actually catches typos, and downstream `match` arms are
//! exhaustively checked at compile time.
//!
//! True provider pass-through (HTTP headers, gateway-specific knobs)
//! goes through `ModelInfo.extra_body` (Layer 1) or, for headers
//! specifically, `client_options.headers`. There is no need for a
//! generic "anything goes" map.

use crate::secret::RedactedSecret;
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt;

/// A single custom-header value. Two authoring modes:
///
/// - [`HeaderValue::Literal`] — a plain JSON string. The value is sent
///   verbatim, with **no** variable expansion. This is the simplified,
///   default form and is byte-for-byte the historical behavior.
/// - [`HeaderValue::Templated`] — `{ "template": "..." }`. The string
///   is expanded against built-in variables (`${SESSION_ID}`,
///   `${MODEL_ID}`, …) at provider-build time by
///   `coco_inference::header_template`.
///
/// Variables use **mandatory braces** (`${NAME}`); a bare `$` is a
/// literal, so a JSON document used as a header value (`{"sid":"${SESSION_ID}"}`)
/// needs no escaping of its own `{`/`}`. Templates are stored
/// unexpanded here so the `ProviderClientFingerprint` reflects config
/// identity (stable within a session); expansion happens downstream.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HeaderValue {
    /// Mode 2 — sent verbatim, no expansion.
    Literal(String),
    /// Mode 1 — expanded against built-in variables.
    Templated { template: String },
}

impl HeaderValue {
    /// Construct a literal (no-expansion) value.
    pub fn literal(value: impl Into<String>) -> Self {
        Self::Literal(value.into())
    }

    /// Construct a templated value.
    pub fn templated(template: impl Into<String>) -> Self {
        Self::Templated {
            template: template.into(),
        }
    }

    /// The raw configured string — either the literal value or the
    /// unexpanded template body. Used by the fingerprint digest.
    pub fn raw(&self) -> &str {
        match self {
            Self::Literal(s) => s,
            Self::Templated { template } => template,
        }
    }

    /// Whether this value carries a template requiring expansion.
    pub fn is_templated(&self) -> bool {
        matches!(self, Self::Templated { .. })
    }
}

impl From<&str> for HeaderValue {
    fn from(value: &str) -> Self {
        Self::Literal(value.to_string())
    }
}

impl From<String> for HeaderValue {
    fn from(value: String) -> Self {
        Self::Literal(value)
    }
}

/// Wire format — `BTreeMap`-ordered, every field `Option`.
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields, rename_all = "snake_case")]
pub struct PartialProviderClientOptions {
    /// Custom HTTP headers (e.g. `X-Corp-Tenant`, gateway tracking, `api-version`).
    /// `BTreeMap` so order is stable across restarts. Each value is
    /// either a literal string or a `{ "template": "..." }` object — see
    /// [`HeaderValue`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, HeaderValue>>,
    /// Anthropic bearer token; if set, `api_key` is ignored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_token: Option<RedactedSecret>,
    /// OpenAI `OpenAI-Organization` header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization_id: Option<String>,
    /// OpenAI `OpenAI-Project` header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// Whether to request `stream_options.include_usage` (OpenAI-compat).
    /// `None` matches the SDK's `false` default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_usage: Option<bool>,
    /// Treat `base_url` as the complete endpoint (skip path suffix).
    /// For Azure-style routing where the path includes deployment + api-version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_url: Option<bool>,
    /// Whether the provider supports `response_format = json_schema`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_structured_outputs: Option<bool>,
    /// Hard idle-timeout backstop (seconds) for the streaming response: abort
    /// the turn if no SSE event arrives within this window. Guards against a
    /// half-open socket the reqwest transport timeout can't catch. `None` or a
    /// non-positive value disables it (the default — the soft 30s stall warn
    /// still fires). Set generously (≥ the longest legitimate inter-event gap;
    /// codex uses 300s).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_idle_timeout_secs: Option<i64>,
}

impl fmt::Debug for PartialProviderClientOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PartialProviderClientOptions")
            .field("headers", &self.headers)
            .field(
                "auth_token",
                &self.auth_token.as_ref().map(|_| "<redacted>"),
            )
            .field("organization_id", &self.organization_id)
            .field("project_id", &self.project_id)
            .field("include_usage", &self.include_usage)
            .field("full_url", &self.full_url)
            .field(
                "supports_structured_outputs",
                &self.supports_structured_outputs,
            )
            .field("stream_idle_timeout_secs", &self.stream_idle_timeout_secs)
            .finish()
    }
}

/// Resolved form — required fields concrete; only genuinely-optional fields stay `Option`.
#[derive(Clone, Default, Serialize)]
pub struct ProviderClientOptions {
    pub headers: BTreeMap<String, HeaderValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_token: Option<RedactedSecret>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organization_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// `None` = SDK default (false for OpenAI-compat).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_usage: Option<bool>,
    /// Default false.
    pub full_url: bool,
    /// Default false.
    pub supports_structured_outputs: bool,
    /// Streaming idle-timeout backstop (seconds). `None` / non-positive =
    /// disabled (default). See [`PartialProviderClientOptions::stream_idle_timeout_secs`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_idle_timeout_secs: Option<i64>,
}

impl ProviderClientOptions {
    /// Construct a resolved form from a partial overlay (no base layer).
    pub fn from_partial(partial: &PartialProviderClientOptions) -> Self {
        Self {
            headers: partial.headers.clone().unwrap_or_default(),
            auth_token: partial.auth_token.clone(),
            organization_id: partial.organization_id.clone(),
            project_id: partial.project_id.clone(),
            include_usage: partial.include_usage,
            full_url: partial.full_url.unwrap_or(false),
            supports_structured_outputs: partial.supports_structured_outputs.unwrap_or(false),
            stream_idle_timeout_secs: partial.stream_idle_timeout_secs,
        }
    }

    /// Layer a partial overlay over `self`. Each `Some` field wins; each `None`
    /// keeps the prior value. `headers` merges key-by-key (overlay wins per key).
    pub fn merge_partial(&mut self, partial: &PartialProviderClientOptions) {
        if let Some(headers) = &partial.headers {
            for (k, v) in headers {
                self.headers.insert(k.clone(), v.clone());
            }
        }
        if let Some(token) = &partial.auth_token {
            self.auth_token = Some(token.clone());
        }
        if let Some(org) = &partial.organization_id {
            self.organization_id = Some(org.clone());
        }
        if let Some(project) = &partial.project_id {
            self.project_id = Some(project.clone());
        }
        if let Some(include) = partial.include_usage {
            self.include_usage = Some(include);
        }
        if let Some(full) = partial.full_url {
            self.full_url = full;
        }
        if let Some(structured) = partial.supports_structured_outputs {
            self.supports_structured_outputs = structured;
        }
        if let Some(idle) = partial.stream_idle_timeout_secs {
            self.stream_idle_timeout_secs = Some(idle);
        }
    }
}

impl fmt::Debug for ProviderClientOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderClientOptions")
            .field("headers", &self.headers)
            .field(
                "auth_token",
                &self.auth_token.as_ref().map(|_| "<redacted>"),
            )
            .field("organization_id", &self.organization_id)
            .field("project_id", &self.project_id)
            .field("include_usage", &self.include_usage)
            .field("full_url", &self.full_url)
            .field(
                "supports_structured_outputs",
                &self.supports_structured_outputs,
            )
            .field("stream_idle_timeout_secs", &self.stream_idle_timeout_secs)
            .finish()
    }
}

#[cfg(test)]
#[path = "client_options.test.rs"]
mod tests;
