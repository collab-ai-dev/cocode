use super::*;

#[test]
fn test_noop_provider() {
    let provider = NoopProvider::new();
    assert_eq!(provider.name(), "noop");
    assert!(!provider.is_available());
}

#[test]
fn test_parse_llm_response() {
    let json = r#"{
        "translated": "user authentication function",
        "intent": "definition",
        "rewritten": "user authentication function",
        "expansions": [
            {"text": "getUserAuth", "type": "camel_case", "weight": 0.9},
            {"text": "auth", "type": "abbreviation", "weight": 0.7}
        ],
        "confidence": 0.92
    }"#;

    let response = LlmRewriteResponse::parse(json).unwrap();
    assert_eq!(response.translated, "user authentication function");
    assert_eq!(response.intent, "definition");
    assert_eq!(response.expansions.len(), 2);
    assert_eq!(response.confidence, 0.92);
}

#[test]
fn test_parse_llm_response_with_surrounding_text() {
    let json = r#"Here is the result:
    {"translated": "test", "intent": "general", "rewritten": "test", "expansions": [], "confidence": 0.8}
    Hope this helps!"#;

    let response = LlmRewriteResponse::parse(json).unwrap();
    assert_eq!(response.translated, "test");
}

#[test]
fn test_to_rewritten_query() {
    let response = LlmRewriteResponse {
        translated: "user authentication".to_string(),
        intent: "definition".to_string(),
        rewritten: "user auth".to_string(),
        expansions: vec![LlmExpansion {
            text: "login".to_string(),
            expansion_type: "synonym".to_string(),
            weight: 0.8,
        }],
        confidence: 0.9,
    };

    let original = "用户认证";
    let result = response.to_rewritten_query(original, 100);

    assert_eq!(result.original, original);
    assert_eq!(result.rewritten, "user auth");
    assert!(result.was_translated);
    assert_eq!(result.source_language, Some("zh".to_string()));
    assert_eq!(result.intent, QueryIntent::Definition);
    assert_eq!(result.expansions.len(), 1);
    assert_eq!(result.source, RewriteSource::Llm);
    assert_eq!(result.latency_ms, 100);
}

#[test]
fn test_extract_json() {
    assert_eq!(extract_json(r#"{"a":1}"#), r#"{"a":1}"#);
    assert_eq!(extract_json(r#"Here is JSON: {"a":1} done"#), r#"{"a":1}"#);
    assert_eq!(extract_json(r#"  {"a":1}  "#), r#"{"a":1}"#);
}

#[test]
fn test_extract_json_nested() {
    // Nested object - the old rfind('}') approach would fail here
    let nested = r#"{"outer": {"inner": 1}}"#;
    assert_eq!(extract_json(nested), nested);

    // Deeply nested with surrounding text
    let deep_nested = r#"Result: {"a": {"b": {"c": 1}}} end"#;
    assert_eq!(extract_json(deep_nested), r#"{"a": {"b": {"c": 1}}}"#);

    // Nested with arrays
    let with_array = r#"{"items": [{"id": 1}, {"id": 2}]}"#;
    assert_eq!(extract_json(with_array), with_array);

    // String containing braces (should not confuse depth tracking)
    let string_braces = r#"{"text": "hello { world }"}"#;
    assert_eq!(extract_json(string_braces), string_braces);

    // Escaped quotes in string
    let escaped = r#"{"text": "say \"hello\""}"#;
    assert_eq!(extract_json(escaped), escaped);

    // Multiple objects - should return first valid one
    let multiple = r#"First: {"a":1} Second: {"b":2}"#;
    assert_eq!(extract_json(multiple), r#"{"a":1}"#);
}

#[test]
fn test_extract_json_llm_response_format() {
    // Typical LLM response with nested expansions
    let llm_response = r#"Here is the analysis:
    {
        "translated": "user authentication",
        "intent": "definition",
        "rewritten": "user auth function",
        "expansions": [
            {"text": "auth", "type": "abbreviation", "weight": 0.8},
            {"text": "login", "type": "synonym", "weight": 0.7}
        ],
        "confidence": 0.92
    }
    Hope this helps!"#;

    let extracted = extract_json(llm_response);
    // Should successfully extract the full JSON including nested arrays
    assert!(extracted.contains("\"expansions\""));
    assert!(extracted.contains("\"abbreviation\""));
    assert!(extracted.contains("\"synonym\""));

    // Verify it's valid JSON
    let parsed: serde_json::Value = serde_json::from_str(extracted).unwrap();
    assert_eq!(parsed["confidence"], 0.92);
    assert_eq!(parsed["expansions"].as_array().unwrap().len(), 2);
}
