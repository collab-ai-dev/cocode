use super::*;
use coco_types::SideQueryStopReason;
use coco_types::SideQueryToolUse;
use coco_types::SideQueryUsage;
use pretty_assertions::assert_eq;

fn names(parse: RecallSelection) -> Vec<String> {
    match parse {
        RecallSelection::Parsed(v) => v,
        RecallSelection::Malformed => panic!("expected Parsed, got Malformed"),
    }
}

#[test]
fn parses_object_with_selected_memories_array() {
    let resp = r#"{"selected_memories": ["a.md", "b.md"]}"#;
    assert_eq!(
        names(parse_selection_response(resp)),
        vec!["a.md".to_string(), "b.md".to_string()]
    );
}

#[test]
fn parses_legal_empty_selection_without_fallback() {
    // A model legitimately answering "no relevant memories" with an
    // empty `selected_memories` array MUST parse as `Parsed(vec![])`
    // — not `Malformed`. The runtime relies on this to avoid a
    // wasted forced-tool retry on every no-match turn.
    let resp = r#"{"selected_memories": []}"#;
    assert_eq!(
        parse_selection_response(resp),
        RecallSelection::Parsed(Vec::new())
    );
}

#[test]
fn parses_legal_json_without_selected_memories_field() {
    // The model emitted legal JSON but no `selected_memories` field
    // (e.g. answered with a free-form reason). Treat as legitimate
    // empty verdict, not malformed — the JSON parsed cleanly, so a
    // forced-tool retry would just spend another LLM call to get the
    // same "nothing relevant" answer.
    let resp = r#"{"reason": "nothing relevant"}"#;
    assert_eq!(
        parse_selection_response(resp),
        RecallSelection::Parsed(Vec::new())
    );
}

#[test]
fn falls_back_to_bare_array() {
    let resp = "[\"a.md\", \"b.md\"]";
    assert_eq!(
        names(parse_selection_response(resp)),
        vec!["a.md".to_string(), "b.md".to_string()]
    );
}

#[test]
fn rescues_markdown_wrapped_bare_array() {
    let resp = "Sure! Here are the most relevant memories:\n\n```json\n[\"a.md\"]\n```\n";
    assert_eq!(
        names(parse_selection_response(resp)),
        vec!["a.md".to_string()]
    );
}

#[test]
fn truncated_json_is_recovered_via_repair() {
    // Behavior change after `coco_utils_json_repair` integration:
    // truncated JSON is closed by the repairer (unbalanced quotes /
    // brackets) and we extract the partial selection. Filenames
    // that don't match the scanned manifest are dropped by
    // `load_relevant_memories`, so a stray half-name from the
    // truncation point causes no harm. Trade-off: fewer wasted
    // forced-tool retries on `max_tokens`-cut responses.
    let resp = r#"{"selected_memories": ["a.md", "b"#;
    let parsed = parse_selection_response(resp);
    match parsed {
        RecallSelection::Parsed(names) => {
            assert!(
                names.contains(&"a.md".to_string()),
                "truncated JSON should still yield the completed entries; got {names:?}"
            );
        }
        RecallSelection::Malformed => panic!("expected repair to recover partial data"),
    }
}

#[test]
fn non_json_text_is_repaired_to_legitimate_empty() {
    // llm_json wraps free-form text as a JSON string; the extractor
    // then sees no `selected_memories` array and returns
    // `Parsed(vec![])` — a legitimate "no matches" verdict that
    // does NOT trigger the forced-tool fallback. The recall ranker
    // saying "nothing relevant" in plain prose should not cost a
    // second LLM call.
    assert_eq!(
        parse_selection_response("not json"),
        RecallSelection::Parsed(Vec::new())
    );
}

#[test]
fn empty_input_is_malformed() {
    assert_eq!(parse_selection_response(""), RecallSelection::Malformed);
    assert_eq!(
        parse_selection_response("   \n  "),
        RecallSelection::Malformed
    );
}

fn mk_response(text: Option<&str>, tool_input: Option<serde_json::Value>) -> SideQueryResponse {
    SideQueryResponse {
        text: text.map(str::to_string),
        tool_uses: tool_input
            .map(|input| {
                vec![SideQueryToolUse {
                    name: "select_memories".into(),
                    input,
                    invalid: false,
                }]
            })
            .unwrap_or_default(),
        stop_reason: SideQueryStopReason::EndTurn,
        usage: SideQueryUsage::default(),
        model_used: "test-model".into(),
    }
}

#[test]
fn extract_prefers_tool_uses_over_text() {
    // When both surfaces are populated, tool_uses wins — that's the
    // canonical structured-output path (Anthropic synthetic json tool,
    // forced tool_choice). Text from the model's chain-of-thought
    // shouldn't shadow the tool input.
    let resp = mk_response(
        Some("some chatter"),
        Some(serde_json::json!({"selected_memories": ["t.md"]})),
    );
    assert_eq!(
        extract_recall_selection(&resp),
        RecallSelection::Parsed(vec!["t.md".into()])
    );
}

#[test]
fn extract_tool_uses_without_field_is_legitimate_empty() {
    // Adapter emitted a tool_use but the model returned `{}`. The
    // JSON object is well-formed; treat as legitimate empty selection,
    // not malformed.
    let resp = mk_response(None, Some(serde_json::json!({})));
    assert_eq!(
        extract_recall_selection(&resp),
        RecallSelection::Parsed(Vec::new())
    );
}

#[test]
fn extract_text_only_routes_through_parse() {
    let resp = mk_response(Some(r#"{"selected_memories": ["x.md"]}"#), None);
    assert_eq!(
        extract_recall_selection(&resp),
        RecallSelection::Parsed(vec!["x.md".into()])
    );
}

#[test]
fn extract_no_content_is_malformed() {
    let resp = mk_response(None, None);
    assert_eq!(extract_recall_selection(&resp), RecallSelection::Malformed);
}

#[test]
fn extract_truncated_text_is_recovered_via_repair() {
    // Behavior change after `coco_utils_json_repair` integration:
    // truncated structured-output text is closed by the repairer
    // and we extract the partial selection.
    let resp = mk_response(Some(r#"{"selected_memories": ["a.md"#), None);
    let parsed = extract_recall_selection(&resp);
    match parsed {
        RecallSelection::Parsed(names) => {
            assert!(
                names.contains(&"a.md".to_string()),
                "truncation repair should yield completed entries; got {names:?}"
            );
        }
        RecallSelection::Malformed => panic!("expected repair to recover partial data"),
    }
}

#[test]
fn prefetch_tracks_surfaced_and_budget() {
    let state = PrefetchState::new();
    assert!(!state.is_surfaced("a.md"));
    state.mark_surfaced("a.md", 100);
    assert!(state.is_surfaced("a.md"));
    assert!(!state.is_budget_exhausted());
    state.mark_surfaced("b.md", MAX_SESSION_BYTES);
    assert!(state.is_budget_exhausted());
}
