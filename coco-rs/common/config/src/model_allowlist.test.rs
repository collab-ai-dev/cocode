use super::is_model_allowed;

fn list(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).to_string()).collect()
}

#[test]
fn absent_allowlist_allows_everything() {
    assert!(is_model_allowed("claude-opus-4-7", None));
}

#[test]
fn empty_allowlist_denies_everything() {
    let available = list(&[]);
    assert!(!is_model_allowed("claude-opus-4-7", Some(&available)));
}

#[test]
fn family_alias_allows_family_when_not_narrowed() {
    let available = list(&["opus"]);
    assert!(is_model_allowed("claude-opus-4-7", Some(&available)));
    assert!(!is_model_allowed("claude-sonnet-4-6", Some(&available)));
}

#[test]
fn specific_family_entry_narrows_family_alias() {
    let available = list(&["opus", "claude-opus-4-5"]);
    assert!(is_model_allowed(
        "claude-opus-4-5-20251101",
        Some(&available)
    ));
    assert!(!is_model_allowed("claude-opus-4-7", Some(&available)));
}

#[test]
fn version_prefix_requires_segment_boundary() {
    let available = list(&["claude-opus-4-5"]);
    assert!(is_model_allowed(
        "claude-opus-4-5-20251101",
        Some(&available)
    ));
    assert!(!is_model_allowed("claude-opus-4-50", Some(&available)));
}

#[test]
fn provider_prefix_is_ignored_for_matching() {
    let available = list(&["opus-4-7"]);
    assert!(is_model_allowed(
        "anthropic/claude-opus-4-7",
        Some(&available)
    ));
}
