use super::is_model_allowed;

fn list(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).to_string()).collect()
}

#[test]
fn absent_allowlist_allows_everything() {
    assert!(is_model_allowed("anthropic", "claude-opus-4-7", None));
}

#[test]
fn empty_allowlist_denies_everything() {
    let available = list(&[]);
    assert!(!is_model_allowed(
        "anthropic",
        "claude-opus-4-7",
        Some(&available)
    ));
}

#[test]
fn full_provider_model_name_allows_exact_match() {
    let available = list(&["anthropic/claude-opus-4-7"]);
    assert!(is_model_allowed(
        "anthropic",
        "claude-opus-4-7",
        Some(&available)
    ));
}

#[test]
fn bare_model_name_is_not_allowed() {
    let available = list(&["claude-opus-4-7"]);
    assert!(!is_model_allowed(
        "anthropic",
        "claude-opus-4-7",
        Some(&available)
    ));
}

#[test]
fn provider_must_match() {
    let available = list(&["openai/claude-opus-4-7"]);
    assert!(!is_model_allowed(
        "anthropic",
        "claude-opus-4-7",
        Some(&available)
    ));
}

#[test]
fn prefix_or_family_alias_is_not_allowed() {
    let available = list(&["anthropic/claude-opus", "anthropic/opus"]);
    assert!(!is_model_allowed(
        "anthropic",
        "claude-opus-4-7",
        Some(&available)
    ));
}
