//! Per-role fallback chain + fallback policy.
//!
//! Each `ModelRole` binds to a `RoleSlots<T>`: a primary plus an
//! ordered list of fallbacks the runtime walks on capacity errors.
//!
//! # Shapes
//!
//! Both `T = ProviderModelSelection` (JSON config side) and `T = ModelSpec`
//! (runtime-resolved side) reuse this one generic, avoiding a parallel
//! type pair. Only the `ProviderModelSelection` instantiation has a custom
//! deserializer — the runtime side is only ever built programmatically
//! by the runtime-config resolver.
//!
//! # JSON shapes accepted for `RoleSlots<ProviderModelSelection>`
//!
//! 1. Bare string: `"anthropic/claude-opus-4-6"` — splits on `/` into
//!    `(provider, model_id)`.
//! 2. Single fallback:
//!    `{ "primary": { "provider": …, "model_id": … }, "fallback": …, "policy": …? }`.
//! 3. Plural fallbacks:
//!    `{ "primary": { "provider": …, "model_id": … }, "fallbacks": [ … ], "policy": …? }`.
//!
//! Shape (1) produces `RoleSlots { primary, fallbacks: vec![], policy: default }`.
//! Shapes (2) and (3) cannot be combined in the same entry — specifying
//! both `fallback` and `fallbacks` is a hard deserialization error. The
//! nested form uses `deny_unknown_fields` so typos in field names
//! surface immediately with actionable messages instead of silently
//! falling through to another variant.

use std::time::Duration;

use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;
use serde::de::Error;
use serde_json::Map;
use serde_json::Value;

use coco_types::ProviderModelSelection;

/// Per-role primary + ordered fallback chain + fallback policy.
///
/// Generic over `T` so the config-facing (`ProviderModelSelection`) and
/// runtime-facing (`ModelSpec`) instantiations share code. Keeping a
/// single type avoids drift between the two sides and mirrors the
/// existing `ModelResult<T>`-style generics in the codebase.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RoleSlots<T> {
    pub primary: T,
    /// Ordered fallbacks. Empty = no fallback configured.
    pub fallbacks: Vec<T>,
    /// Policy for fallback-chain exhaustion and primary recovery probes.
    pub policy: FallbackPolicy,
}

impl<T> RoleSlots<T> {
    pub fn new(primary: T) -> Self {
        Self {
            primary,
            fallbacks: Vec::new(),
            policy: FallbackPolicy::default(),
        }
    }

    pub fn with_fallback(mut self, fallback: T) -> Self {
        self.fallbacks.push(fallback);
        self
    }

    pub fn with_fallbacks(mut self, fallbacks: Vec<T>) -> Self {
        self.fallbacks = fallbacks;
        self
    }

    pub fn with_policy(mut self, policy: FallbackPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Map both primary and fallbacks with a single closure.
    ///
    /// Used by the runtime-config resolver to lift
    /// `RoleSlots<ProviderModelSelection>` (config-side) into
    /// `RoleSlots<ModelSpec>` (runtime-side) by resolving each
    /// selection against the provider catalog.
    pub fn try_map<U, E, F>(self, mut f: F) -> Result<RoleSlots<U>, E>
    where
        F: FnMut(T) -> Result<U, E>,
    {
        let primary = f(self.primary)?;
        let fallbacks = self
            .fallbacks
            .into_iter()
            .map(&mut f)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(RoleSlots {
            primary,
            fallbacks,
            policy: self.policy,
        })
    }
}

/// Complete fallback policy for a role runtime.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FallbackPolicy {
    pub exhausted_retry: ExhaustedRetryPolicy,
    pub recovery: RecoveryProbePolicy,
}

/// Controlled retry after every slot in a fallback chain has failed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExhaustedRetryPolicy {
    /// Total number of full-chain cycles before surfacing the last
    /// capacity/rate-limit error. Clamped to at least 1.
    pub max_cycles: i32,
    /// Seconds before the first retry cycle.
    pub initial_backoff_secs: u64,
    /// Upper bound on backoff in seconds.
    pub max_backoff_secs: u64,
}

impl Default for ExhaustedRetryPolicy {
    fn default() -> Self {
        Self {
            max_cycles: 2,
            initial_backoff_secs: 2,
            max_backoff_secs: 30,
        }
    }
}

impl ExhaustedRetryPolicy {
    pub fn max_cycles(&self) -> i32 {
        self.max_cycles.max(1)
    }

    pub fn initial_backoff(&self) -> Duration {
        Duration::from_secs(self.initial_backoff_secs)
    }

    pub fn max_backoff(&self) -> Duration {
        Duration::from_secs(self.max_backoff_secs.max(self.initial_backoff_secs))
    }
}

/// Half-open recovery probe policy: after switching to a fallback,
/// periodically probe the primary. Backoff doubles on each probe
/// failure up to `max_backoff`; `max_attempts` caps total probes per
/// session. `max_attempts = 0` disables recovery probes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RecoveryProbePolicy {
    /// Seconds before the first probe. Also the initial backoff.
    pub initial_backoff_secs: u64,
    /// Upper bound on backoff in seconds.
    pub max_backoff_secs: u64,
    /// Maximum probe attempts per session. Clamped to at least 0.
    pub max_attempts: i32,
}

impl Default for RecoveryProbePolicy {
    fn default() -> Self {
        Self {
            initial_backoff_secs: 60,
            max_backoff_secs: 1_800,
            max_attempts: 10,
        }
    }
}

impl RecoveryProbePolicy {
    pub fn initial_backoff(&self) -> Duration {
        Duration::from_secs(self.initial_backoff_secs)
    }

    pub fn max_backoff(&self) -> Duration {
        Duration::from_secs(self.max_backoff_secs.max(self.initial_backoff_secs))
    }

    pub fn max_attempts(&self) -> i32 {
        self.max_attempts.max(0)
    }
}

// ─── Deserializer for RoleSlots<ProviderModelSelection> ─────────────────────

impl<'de> Deserialize<'de> for RoleSlots<ProviderModelSelection> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Dispatch explicitly on the observed JSON shape instead of
        // relying on serde's untagged fallthrough. Routing on presence of
        // `primary`/`fallback`/`fallbacks`/`policy` keys is
        // deterministic and yields actionable error messages.
        let value = Value::deserialize(d)?;

        if let Some(s) = value.as_str() {
            return parse_bare_string(s).map_err(D::Error::custom);
        }

        let obj = value
            .as_object()
            .ok_or_else(|| D::Error::custom("role selection must be a string or nested object"))?;

        let has_nested_keys = obj.contains_key("primary")
            || obj.contains_key("fallback")
            || obj.contains_key("fallbacks")
            || obj.contains_key("policy")
            || obj.contains_key("recovery");

        if has_nested_keys {
            reject_unknown_fields::<D::Error>(
                obj,
                &["primary", "fallback", "fallbacks", "policy"],
                "nested role selection",
            )?;
            let primary = obj
                .get("primary")
                .ok_or_else(|| D::Error::custom("nested role selection requires `primary`"))
                .and_then(|v| parse_selection_value::<D::Error>(v, "primary"))?;
            let fallback = obj
                .get("fallback")
                .map(|v| parse_selection_value::<D::Error>(v, "fallback"))
                .transpose()?;
            let fallback_list = obj
                .get("fallbacks")
                .map(parse_fallbacks::<D::Error>)
                .transpose()?;
            let policy = obj
                .get("policy")
                .map(|v| serde_json::from_value(v.clone()).map_err(D::Error::custom))
                .transpose()?
                .unwrap_or_default();
            let fallbacks = match (fallback, fallback_list) {
                (Some(_), Some(_)) => {
                    return Err(D::Error::custom(
                        "use either `fallback` (single) or `fallbacks` (list), not both",
                    ));
                }
                (Some(one), None) => vec![one],
                (None, Some(list)) => list,
                (None, None) => Vec::new(),
            };
            Ok(RoleSlots {
                primary,
                fallbacks,
                policy,
            })
        } else {
            Err(D::Error::custom(
                "role selection object must use nested form with `primary`",
            ))
        }
    }
}

/// Emit the compact nested form on serialize. Round-tripping a
/// bare-string-form config through serde produces the nested form —
/// acceptable because the nested form is always valid input.
impl Serialize for RoleSlots<ProviderModelSelection> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("RoleSlots", 3)?;
        st.serialize_field("primary", &self.primary)?;
        if !self.fallbacks.is_empty() {
            st.serialize_field("fallbacks", &self.fallbacks)?;
        } else {
            st.skip_field("fallbacks")?;
        }
        if self.policy != FallbackPolicy::default() {
            st.serialize_field("policy", &self.policy)?;
        } else {
            st.skip_field("policy")?;
        }
        st.end()
    }
}

fn parse_bare_string(s: &str) -> Result<RoleSlots<ProviderModelSelection>, String> {
    ProviderModelSelection::from_slash_str(s).map(RoleSlots::new)
}

fn parse_fallbacks<E: Error>(value: &Value) -> Result<Vec<ProviderModelSelection>, E> {
    let values = value
        .as_array()
        .ok_or_else(|| E::custom("`fallbacks` must be an array"))?;
    values
        .iter()
        .enumerate()
        .map(|(idx, v)| parse_selection_value(v, &format!("fallbacks[{idx}]")))
        .collect()
}

fn parse_selection_value<E: Error>(
    value: &Value,
    label: &str,
) -> Result<ProviderModelSelection, E> {
    let obj = value
        .as_object()
        .ok_or_else(|| E::custom(format!("`{label}` must be an object")))?;
    parse_selection_object(obj, label)
}

fn parse_selection_object<E: Error>(
    obj: &Map<String, Value>,
    label: &str,
) -> Result<ProviderModelSelection, E> {
    reject_unknown_fields::<E>(obj, &["provider", "model_id"], label)?;
    let provider = required_non_empty_string::<E>(obj, "provider", label)?;
    let model_id = required_non_empty_string::<E>(obj, "model_id", label)?;
    Ok(ProviderModelSelection { provider, model_id })
}

fn required_non_empty_string<E: Error>(
    obj: &Map<String, Value>,
    field: &str,
    label: &str,
) -> Result<String, E> {
    let value = obj
        .get(field)
        .ok_or_else(|| E::custom(format!("{label} must include `{field}`")))?;
    let s = value
        .as_str()
        .ok_or_else(|| E::custom(format!("{label}.{field} must be a string")))?;
    if s.is_empty() {
        return Err(E::custom(format!("{label}.{field} must be non-empty")));
    }
    Ok(s.to_string())
}

fn reject_unknown_fields<E: Error>(
    obj: &Map<String, Value>,
    allowed: &[&str],
    label: &str,
) -> Result<(), E> {
    if let Some(field) = obj.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(E::custom(format!(
            "{label} contains unknown field `{field}`"
        )));
    }
    Ok(())
}

#[cfg(test)]
#[path = "role_slots.test.rs"]
mod tests;
