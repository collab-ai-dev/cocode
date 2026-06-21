//! Fast mode = same model, faster output speed (Anthropic "penguin mode") —
//! NOT a model switch. Whether a model supports it is a per-model capability
//! (`Capability::FastMode`).
//!
//! Only the capability gate lives here — a pure, provider-agnostic helper.
//! Runtime/session state (cooldown, per-session opt-in, org availability) is
//! intentionally NOT a process global. When that completeness work lands
//! (config#247 deferred items), it must be held as **instance** state on the
//! owning session (e.g. `SessionRuntime`) and passed down — never a singleton.
//! The previously-dead `fast_mode_global()` singleton and its
//! cooldown/org/availability/opt-in API (0 production readers) were deleted to
//! remove the shared-mutable-global smell at the root rather than serialize the
//! tests around it.

/// Whether `model_id` supports fast mode — **capability-driven and
/// provider-agnostic** (config#247). Any model that declares
/// [`Capability::FastMode`](coco_types::Capability::FastMode) qualifies,
/// regardless of provider; the owning provider crate translates the resolved
/// flag to its wire option (Anthropic → `speed=fast`). This is the L0
/// (builtin-registry) lookup used by the TUI toggle — the same source the TUI
/// model picker uses. The per-turn engine gate checks the fully-resolved
/// `ModelInfo::has_capability` on the active snapshot directly.
///
/// Replaces the old `model_id.contains("opus-4-6")` substring, which matched
/// **no** shipped builtin (`claude-sonnet-4-6` / `claude-opus-4-7` /
/// `claude-haiku-4-5`) and so disabled fast mode for every model.
pub fn is_fast_mode_supported_by_model(model_id: &str) -> bool {
    crate::builtin::builtin_models_partial()
        .get(model_id)
        .and_then(|info| info.capabilities.as_ref())
        .is_some_and(|caps| caps.contains(&coco_types::Capability::FastMode))
}

#[cfg(test)]
#[path = "fast_mode.test.rs"]
mod tests;
