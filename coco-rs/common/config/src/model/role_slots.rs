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
use coco_types::ReasoningEffort;

/// One position in a role's model chain: a model identity plus the
/// reasoning effort to use **while that specific model is serving**.
///
/// Effort rides the slot, not the role, so a fallback model can run at
/// a different thinking level than the primary — whichever slot is live
/// contributes its own effort at the wire. See
/// [`super::ModelInfo::resolve_thinking_level`] for how the effort is
/// clamped against the serving model's declared ladder.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RoleSlot<T> {
    pub model: T,
    /// Per-slot reasoning effort intent. `None` = this slot states no
    /// effort, so the wire layer falls through to the model's
    /// `default_thinking_level`, then the provider default. `None` is
    /// distinct from `Some(ReasoningEffort::Off)` (explicit "thinking
    /// off") — the latter suppresses thinking, the former defers.
    pub effort: Option<ReasoningEffort>,
}

impl<T> RoleSlot<T> {
    /// Slot with no effort declared — model default applies downstream.
    pub fn bare(model: T) -> Self {
        Self {
            model,
            effort: None,
        }
    }
}

/// Per-role primary + ordered fallback chain + fallback policy.
///
/// Generic over `T` so the config-facing (`ProviderModelSelection`) and
/// runtime-facing (`ModelSpec`) instantiations share code. Keeping a
/// single type avoids drift between the two sides and mirrors the
/// existing `ModelResult<T>`-style generics in the codebase. Each slot
/// (primary and every fallback) carries its own per-slot [`RoleSlot`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RoleSlots<T> {
    pub primary: RoleSlot<T>,
    /// Ordered fallbacks. Empty = no fallback configured.
    pub fallbacks: Vec<RoleSlot<T>>,
    /// Policy for fallback-chain exhaustion and primary recovery probes.
    pub policy: FallbackPolicy,
}

impl<T> RoleSlots<T> {
    pub fn new(primary: T) -> Self {
        Self {
            primary: RoleSlot::bare(primary),
            fallbacks: Vec::new(),
            policy: FallbackPolicy::default(),
        }
    }

    pub fn with_fallback(mut self, fallback: T) -> Self {
        self.fallbacks.push(RoleSlot::bare(fallback));
        self
    }

    pub fn with_fallbacks(mut self, fallbacks: Vec<T>) -> Self {
        self.fallbacks = fallbacks.into_iter().map(RoleSlot::bare).collect();
        self
    }

    pub fn with_policy(mut self, policy: FallbackPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Map every slot's model with a single closure, carrying each
    /// slot's effort through unchanged.
    ///
    /// Used by the runtime-config resolver to lift
    /// `RoleSlots<ProviderModelSelection>` (config-side) into
    /// `RoleSlots<ModelSpec>` (runtime-side) by resolving each
    /// selection against the provider catalog.
    pub fn try_map<U, E, F>(self, mut f: F) -> Result<RoleSlots<U>, E>
    where
        F: FnMut(T) -> Result<U, E>,
    {
        let primary = RoleSlot {
            model: f(self.primary.model)?,
            effort: self.primary.effort,
        };
        let fallbacks = self
            .fallbacks
            .into_iter()
            .map(|slot| {
                Ok(RoleSlot {
                    model: f(slot.model)?,
                    effort: slot.effort,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(RoleSlots {
            primary,
            fallbacks,
            policy: self.policy,
        })
    }
}

impl<T: Clone> RoleSlots<T> {
    /// Clone the chain but drop every slot's effort. Used when an
    /// unconfigured role borrows Main's **models** at config-resolution
    /// time — effort must not ride along, since it belongs only to the
    /// role that explicitly declared it. A role that wants an effort
    /// must configure `models.<role>` itself.
    pub fn without_effort(&self) -> Self {
        Self {
            primary: RoleSlot::bare(self.primary.model.clone()),
            fallbacks: self
                .fallbacks
                .iter()
                .map(|slot| RoleSlot::bare(slot.model.clone()))
                .collect(),
            policy: self.policy,
        }
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
            return ProviderModelSelection::from_slash_str(s)
                .map(RoleSlots::new)
                .map_err(D::Error::custom);
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
            // The most likely misplacement: `effort` at the role level.
            // It belongs on a slot object — give a guiding error instead
            // of the generic unknown-field one.
            if obj.contains_key("effort") {
                return Err(D::Error::custom(
                    "`effort` belongs on a slot object, not at the role level — \
                     e.g. \"primary\": {\"provider\": \"openai\", \"model_id\": \"gpt-5\", \
                     \"effort\": \"high\"}",
                ));
            }
            reject_unknown_fields::<D::Error>(
                obj,
                &["primary", "fallback", "fallbacks", "policy"],
                "nested role selection",
            )?;
            let primary = obj
                .get("primary")
                .ok_or_else(|| D::Error::custom("nested role selection requires `primary`"))
                .and_then(|v| parse_slot_value::<D::Error>(v, "primary"))?;
            let fallback = obj
                .get("fallback")
                .map(|v| parse_slot_value::<D::Error>(v, "fallback"))
                .transpose()?;
            let fallback_list = obj
                .get("fallbacks")
                .map(parse_slot_fallbacks::<D::Error>)
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

/// Emit a slot as a flat object `{provider, model_id, effort?}`.
/// `effort` is skipped when absent so a slot that states no effort
/// round-trips to the same minimal shape it was written in.
impl Serialize for RoleSlot<ProviderModelSelection> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("RoleSlot", 2 + usize::from(self.effort.is_some()))?;
        st.serialize_field("provider", &self.model.provider)?;
        st.serialize_field("model_id", &self.model.model_id)?;
        if let Some(effort) = self.effort {
            st.serialize_field("effort", &effort)?;
        } else {
            st.skip_field("effort")?;
        }
        st.end()
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

fn parse_slot_fallbacks<E: Error>(
    value: &Value,
) -> Result<Vec<RoleSlot<ProviderModelSelection>>, E> {
    let values = value
        .as_array()
        .ok_or_else(|| E::custom("`fallbacks` must be an array"))?;
    values
        .iter()
        .enumerate()
        .map(|(idx, v)| parse_slot_value(v, &format!("fallbacks[{idx}]")))
        .collect()
}

/// Parse one slot: either a `"provider/model_id"` shorthand (no effort)
/// or an object `{provider, model_id, effort?}`.
fn parse_slot_value<E: Error>(
    value: &Value,
    label: &str,
) -> Result<RoleSlot<ProviderModelSelection>, E> {
    if let Some(s) = value.as_str() {
        return ProviderModelSelection::from_slash_str(s)
            .map(RoleSlot::bare)
            .map_err(E::custom);
    }
    let obj = value.as_object().ok_or_else(|| {
        E::custom(format!(
            "`{label}` must be a `provider/model_id` string or an object"
        ))
    })?;
    reject_unknown_fields::<E>(obj, &["provider", "model_id", "effort"], label)?;
    let provider = required_non_empty_string::<E>(obj, "provider", label)?;
    let model_id = required_non_empty_string::<E>(obj, "model_id", label)?;
    let effort = parse_slot_effort::<E>(obj, label)?;
    Ok(RoleSlot {
        model: ProviderModelSelection { provider, model_id },
        effort,
    })
}

/// Parse the optional `effort` key on a slot object. Absent or `null`
/// yields `None` (defer to model default); a string is parsed against
/// [`ReasoningEffort`]'s canonical names + aliases.
fn parse_slot_effort<E: Error>(
    obj: &Map<String, Value>,
    label: &str,
) -> Result<Option<ReasoningEffort>, E> {
    match obj.get("effort") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let s = value.as_str().ok_or_else(|| {
                E::custom(format!(
                    "{label}.effort must be a string \
                     (one of off/auto/minimal/low/medium/high/xhigh)"
                ))
            })?;
            s.parse::<ReasoningEffort>().map(Some).map_err(|_| {
                E::custom(format!(
                    "{label}.effort `{s}` is invalid — expected one of \
                     off/auto/minimal/low/medium/high/xhigh"
                ))
            })
        }
    }
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
