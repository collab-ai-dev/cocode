//! Shared `available_models` matching.
//!
//! TS parity: `utils/model/modelAllowlist.ts`. The setting distinguishes
//! absence from an empty list: absent means allow all, empty means deny all.

const FAMILY_ALIASES: &[&str] = &["opus", "sonnet", "haiku"];

/// Return whether `model` is allowed by `available_models`.
///
/// `None` means the setting is absent and every model is allowed. `Some([])`
/// means the setting is present but empty, so no models are allowed.
pub fn is_model_allowed(model: &str, available_models: Option<&[String]>) -> bool {
    let Some(specs) = available_models else {
        return true;
    };
    if specs.is_empty() {
        return false;
    }

    specs
        .iter()
        .any(|spec| model_matches_spec(model, spec, specs))
}

fn model_matches_spec(model: &str, spec: &str, all_specs: &[String]) -> bool {
    if model.eq_ignore_ascii_case(spec) {
        return true;
    }

    let model = normalize_model(model);
    let spec = normalize_model(spec);

    if model == spec {
        return true;
    }

    if FAMILY_ALIASES.contains(&spec.as_str()) {
        return family_alias_matches(&model, &spec, all_specs);
    }

    prefix_matches_model(&model, &spec)
}

fn family_alias_matches(model: &str, family: &str, all_specs: &[String]) -> bool {
    let needle = format!("-{family}-");
    if !model.contains(&needle) && !model.starts_with(&format!("{family}-")) {
        return false;
    }

    let has_specific = all_specs.iter().any(|spec| {
        let normalized = normalize_model(spec);
        normalized != family && family_model_matches(&normalized, family)
    });
    !has_specific
}

fn family_model_matches(model: &str, family: &str) -> bool {
    let needle = format!("-{family}-");
    model.contains(&needle) || model.starts_with(&format!("{family}-"))
}

fn prefix_matches_model(model: &str, spec: &str) -> bool {
    segment_prefix_matches(model, spec)
        || FAMILY_ALIASES.iter().any(|family| {
            spec.starts_with(&format!("{family}-"))
                && model
                    .find(&format!("-{spec}"))
                    .is_some_and(|idx| segment_prefix_matches(&model[idx + 1..], spec))
        })
}

fn segment_prefix_matches(model: &str, spec: &str) -> bool {
    model.starts_with(spec) && segment_boundary(model, spec.len())
}

fn segment_boundary(value: &str, index: usize) -> bool {
    value.len() == index
        || value
            .as_bytes()
            .get(index)
            .is_some_and(|b| matches!(b, b'-' | b'.' | b'_' | b'/'))
}

fn normalize_model(value: &str) -> String {
    let mut lower = value.trim().to_ascii_lowercase();
    if let Some((_, model)) = lower.rsplit_once('/') {
        lower = model.to_string();
    }
    lower.replace('.', "-")
}

#[cfg(test)]
#[path = "model_allowlist.test.rs"]
mod tests;
