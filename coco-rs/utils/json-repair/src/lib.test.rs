use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn strict_parse_returns_clean() {
    let (v, outcome) = parse_with_repair(r#"{"a": 1}"#).unwrap();
    assert_eq!(v, json!({"a": 1}));
    assert_eq!(outcome, RepairOutcome::Clean);
}

#[test]
fn strict_parse_with_surrounding_whitespace_returns_clean() {
    // Trimming whitespace doesn't count as repair — `serde_json` accepts
    // the trimmed form fine.
    let (v, outcome) = parse_with_repair("  \n  {\"a\": 1}  \n  ").unwrap();
    assert_eq!(v, json!({"a": 1}));
    assert_eq!(outcome, RepairOutcome::Clean);
}

#[test]
fn trailing_comma_is_repaired() {
    let (v, outcome) = parse_with_repair(r#"{"a": 1,}"#).unwrap();
    assert_eq!(v, json!({"a": 1}));
    assert_eq!(outcome, RepairOutcome::Repaired);
}

#[test]
fn missing_closing_brace_is_repaired() {
    let (v, outcome) = parse_with_repair(r#"{"a": 1"#).unwrap();
    assert_eq!(v, json!({"a": 1}));
    assert_eq!(outcome, RepairOutcome::Repaired);
}

#[test]
fn missing_closing_bracket_is_repaired() {
    let (v, outcome) = parse_with_repair(r#"[1, 2, 3"#).unwrap();
    assert_eq!(v, json!([1, 2, 3]));
    assert_eq!(outcome, RepairOutcome::Repaired);
}

#[test]
fn markdown_code_fence_is_stripped() {
    let raw = "```json\n{\"selected\": [\"a.md\"]}\n```";
    let (v, outcome) = parse_with_repair(raw).unwrap();
    assert_eq!(v, json!({"selected": ["a.md"]}));
    assert_eq!(outcome, RepairOutcome::Repaired);
}

#[test]
fn unquoted_keys_are_repaired() {
    let (v, outcome) = parse_with_repair(r#"{a: 1, b: "two"}"#).unwrap();
    assert_eq!(v, json!({"a": 1, "b": "two"}));
    assert_eq!(outcome, RepairOutcome::Repaired);
}

#[test]
fn single_quoted_strings_are_repaired() {
    let (v, outcome) = parse_with_repair(r#"{'a': 'hello'}"#).unwrap();
    assert_eq!(v, json!({"a": "hello"}));
    assert_eq!(outcome, RepairOutcome::Repaired);
}

#[test]
fn empty_input_is_distinguished_error_variant() {
    // Callers (tool-input parsing, recall) want to apply per-domain
    // defaults on empty input — typically `{}` or "no recall" — so
    // the empty case is tagged separately from a malformed repair.
    assert!(matches!(
        parse_with_repair(""),
        Err(JsonRepairError::EmptyInput)
    ));
    assert!(matches!(
        parse_with_repair("   \n  "),
        Err(JsonRepairError::EmptyInput)
    ));
}

#[test]
fn repair_to_string_returns_repaired_text() {
    let (out, outcome) = repair_to_string(r#"{"a": 1,}"#).unwrap();
    assert_eq!(outcome, RepairOutcome::Repaired);
    // Result must be parseable.
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v, json!({"a": 1}));
}

#[test]
fn repair_to_string_clean_returns_trimmed_input() {
    let (out, outcome) = repair_to_string("  {\"a\": 1}  ").unwrap();
    assert_eq!(outcome, RepairOutcome::Clean);
    assert_eq!(out, r#"{"a": 1}"#);
}
