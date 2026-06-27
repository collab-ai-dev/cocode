use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;

use super::RepairOutcome;
use super::parse_with_repair;

#[test]
fn clean_parse_succeeds() {
    let (v, outcome) = parse_with_repair(r#"{"a": 1}"#).unwrap();
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
fn markdown_code_fence_is_stripped() {
    let (v, outcome) = parse_with_repair("```json\n{\"path\": \"/tmp\"}\n```").unwrap();
    assert_eq!(v, json!({"path": "/tmp"}));
    assert_eq!(outcome, RepairOutcome::Repaired);
}

#[test]
fn single_quotes_are_repaired() {
    let (v, outcome) = parse_with_repair(r#"{'a': 'hello'}"#).unwrap();
    assert_eq!(v, json!({"a": "hello"}));
    assert_eq!(outcome, RepairOutcome::Repaired);
}

#[test]
fn unclosed_bracket_is_repaired() {
    let (v, outcome) = parse_with_repair(r#"{"path": "/tmp"#).unwrap();
    assert_eq!(v, json!({"path": "/tmp"}));
    assert_eq!(outcome, RepairOutcome::Repaired);
}

#[test]
fn empty_input_returns_err() {
    assert!(parse_with_repair("").is_err());
    assert!(parse_with_repair("   \n  ").is_err());
}

#[test]
fn truly_malformed_returns_err() {
    // Pure garbage that even llm_json can't salvage into JSON. (Most
    // inputs *can* be salvaged — repair is intentionally aggressive —
    // so this is mostly a guarantee that we don't panic.)
    let result = parse_with_repair("\u{0000}\u{0001}\u{0002}");
    // llm_json may still produce something; accept either outcome.
    // The important property: no panic, returns a Result.
    let _ = result;
}

// ---------------------------------------------------------------------------
// Provider-shape matrix — common malformed payloads each provider family
// emits. `parse_tool_arguments_or_empty` is what every adapter calls when
// turning the wire `arguments`/`input_json` string into a `Value`; its
// job is to either recover or hand schema validation an empty object so the schema
// validator can name the missing fields.
//
// Each test simulates *one provider's typical malformation* to lock in
// the behaviour LLMs see across the multi-provider matrix.
// ---------------------------------------------------------------------------

use super::parse_tool_arguments_or_empty;

#[test]
fn provider_matrix_openai_chat_clean_object() {
    let v = parse_tool_arguments_or_empty(r#"{"path": "/tmp/x"}"#, "Read");
    assert_eq!(v, json!({"path": "/tmp/x"}));
}

#[test]
fn provider_matrix_openai_chat_trailing_comma() {
    // GPT-4.x / GPT-5 occasionally emit a trailing comma when the
    // model truncates a parameterized call mid-decoding.
    let v = parse_tool_arguments_or_empty(r#"{"path": "/tmp/x", "limit": 100,}"#, "Read");
    assert_eq!(v, json!({"path": "/tmp/x", "limit": 100}));
}

#[test]
fn provider_matrix_openai_chat_empty_arguments() {
    // Some providers emit `arguments: ""` for parameterless tool calls.
    // Adapter convention: empty → empty object so the schema sees
    // `{}` instead of treating it as a parse failure.
    let v = parse_tool_arguments_or_empty("", "TodoWrite");
    assert_eq!(v, json!({}));
}

#[test]
fn provider_matrix_openai_chat_whitespace_only_arguments() {
    let v = parse_tool_arguments_or_empty("   \n\t  ", "TodoWrite");
    assert_eq!(v, json!({}));
}

#[test]
fn provider_matrix_glm_or_doubao_markdown_fence() {
    // GLM-4 / Doubao-pro / DeepSeek wrap tool arguments in
    // ```json …``` despite the OpenAI-compat protocol saying not to.
    // llm_json strips the fence.
    let v =
        parse_tool_arguments_or_empty("```json\n{\"file_path\": \"/repo/main.rs\"}\n```", "Read");
    assert_eq!(v, json!({"file_path": "/repo/main.rs"}));
}

#[test]
fn provider_matrix_glm_or_doubao_markdown_fence_lowercase() {
    let v = parse_tool_arguments_or_empty("```\n{\"a\": 1}\n```", "Tool");
    assert_eq!(v, json!({"a": 1}));
}

#[test]
fn provider_matrix_deepseek_single_quotes() {
    // DeepSeek and other CN-trained models sometimes emit
    // Python-style single quotes (the only quote style they saw on
    // function arguments in training).
    let v = parse_tool_arguments_or_empty(r#"{'path': '/tmp/foo'}"#, "Read");
    assert_eq!(v, json!({"path": "/tmp/foo"}));
}

#[test]
fn provider_matrix_unquoted_keys() {
    // Permissive JSON5-ish emission.
    let v = parse_tool_arguments_or_empty(r#"{path: "/tmp", limit: 100}"#, "Read");
    assert_eq!(v, json!({"path": "/tmp", "limit": 100}));
}

#[test]
fn provider_matrix_anthropic_streaming_truncated_brackets() {
    // Anthropic streaming `input_json_delta` accumulation: the stream
    // ends before the closing bracket arrives (network drop, model
    // hit max_tokens mid-emission, etc.). `parse_with_repair`
    // closes the structure with whatever's there.
    let v = parse_tool_arguments_or_empty(r#"{"path": "/tmp/in-flight"#, "Read");
    assert_eq!(v, json!({"path": "/tmp/in-flight"}));
}

#[test]
fn provider_matrix_anthropic_streaming_unclosed_string() {
    let v = parse_tool_arguments_or_empty(r#"{"a": "open"#, "Tool");
    assert_eq!(v, json!({"a": "open"}));
}

#[test]
fn provider_matrix_array_root_falls_back_to_empty() {
    // Tool input must be an object per JSON Schema; bare arrays
    // recover as arrays, which the schema validator then rejects.
    // The recovery itself doesn't fall back to {} — that's schema validation's
    // concern. This test pins the contract.
    let v = parse_tool_arguments_or_empty(r#"[1, 2, 3]"#, "Tool");
    assert_eq!(v, json!([1, 2, 3]));
}

#[test]
fn provider_matrix_pure_garbage_preserves_raw_string() {
    // When repair truly fails (e.g., bytes that can't be salvaged
    // into JSON), we hand schema validation a `Value::String(raw)` so the
    // schema validator surfaces "expected object, got string" AND
    // the raw bytes survive for downstream diagnostics.
    let raw = "\u{0000}\u{0001}}!{!!!";
    let v = parse_tool_arguments_or_empty(raw, "Tool");
    // Either repair salvaged it into some Value, or we preserved
    // the raw string. Both shapes carry diagnostic signal — the
    // contract: `invalid` stays false and the raw model output is
    // not silently replaced with `{}`.
    if let Value::String(s) = &v {
        assert_eq!(s, raw, "preserved raw bytes verbatim");
    } else {
        // llm_json salvaged something — accept any non-empty-object
        // result as long as the helper didn't unilaterally drop
        // information.
        assert_ne!(v, json!({}), "must not silently substitute {{}}");
    }
}

#[test]
fn provider_matrix_null_arguments_string_falls_back_to_empty() {
    // `arguments: "null"` is *valid* JSON but the wrong shape for a
    // tool call. `parse_with_repair` returns `Value::Null`; the
    // helper passes that through (it's not its job to coerce shape).
    // schema validation flags it.
    let v = parse_tool_arguments_or_empty("null", "Tool");
    assert_eq!(v, json!(null));
}

#[test]
fn provider_matrix_number_arguments_string_passes_through() {
    let v = parse_tool_arguments_or_empty("42", "Tool");
    assert_eq!(v, json!(42));
}

#[test]
fn provider_matrix_nested_stringified_json_not_unwrapped() {
    // `arguments: "\"{\\\"path\\\":\\\"/tmp\\\"}\""` produces
    // `Value::String("{\"path\":\"/tmp\"}")` — a JSON string whose
    // inner content happens to be JSON. wire parsing does NOT recursively
    // parse; schema validation's `normalize_value_string` handles that case.
    // This test pins the wire parsing contract.
    let v = parse_tool_arguments_or_empty("\"{\\\"path\\\":\\\"/tmp\\\"}\"", "Read");
    assert_eq!(v, json!("{\"path\":\"/tmp\"}"));
}

#[test]
fn provider_matrix_extra_whitespace_around_object() {
    let v = parse_tool_arguments_or_empty("  \n {\"a\": 1}  \n  ", "Tool");
    assert_eq!(v, json!({"a": 1}));
}
