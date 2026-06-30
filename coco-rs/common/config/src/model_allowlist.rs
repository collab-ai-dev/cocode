//! Shared `available_models` matching.
//!
//! The setting distinguishes absence from an empty list: absent means allow
//! all, empty means deny all. Entries are strict `provider/model_id` full
//! names; this layer never infers provider-specific families or fallbacks.

/// Return whether `model` is allowed by `available_models`.
/// `None` means the setting is absent and every model is allowed. `Some([])`
/// means the setting is present but empty, so no models are allowed.
pub fn is_model_allowed(
    provider: &str,
    model_id: &str,
    available_models: Option<&[String]>,
) -> bool {
    let Some(specs) = available_models else {
        return true;
    };
    if specs.is_empty() {
        return false;
    }

    let full_name = format!("{provider}/{model_id}");
    specs.iter().any(|spec| spec.trim() == full_name)
}

#[cfg(test)]
#[path = "model_allowlist.test.rs"]
mod tests;
