use super::*;

#[test]
fn test_scan_returns_none_for_clean_content() {
    let result = scan_for_secrets("MEMORY.md", "# Notes\n- Reminder: refactor parser\n");
    assert!(result.is_none());
}

#[test]
fn test_scan_returns_match_for_anthropic_key() {
    let body = "API key: sk-ant-api03-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxAA\n";
    let result = scan_for_secrets("MEMORY.md", body).expect("expected anthropic match");
    assert_eq!(result.path, "MEMORY.md");
    assert_eq!(result.rule_id, "anthropic-api-key");
    assert_eq!(result.label, "Anthropic Api Key");
}

#[test]
fn test_label_capitalises_each_segment() {
    assert_eq!(label_from_rule("github-pat"), "Github Pat");
    assert_eq!(label_from_rule("aws-access-token"), "Aws Access Token");
}
