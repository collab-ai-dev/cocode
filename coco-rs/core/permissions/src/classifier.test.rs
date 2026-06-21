use super::*;

const EMPTY_MESSAGES: &[coco_messages::Message] = &[];

#[test]
fn test_is_safe_tool() {
    assert!(is_safe_tool("Read"));
    assert!(is_safe_tool("Glob"));
    assert!(is_safe_tool("TaskCreate"));
    assert!(!is_safe_tool("Bash"));
    assert!(!is_safe_tool("Write"));
    assert!(!is_safe_tool("Edit"));
}

#[test]
fn test_format_action() {
    let action =
        format_action_for_classifier("Bash", &serde_json::json!({"command": "rm -rf /"}), None);
    assert!(action.contains("<action>"));
    assert!(action.contains("tool: Bash"));
    assert!(action.contains("rm -rf"));
}

#[test]
fn test_parse_xml_block_yes() {
    assert_eq!(parse_xml_block("<block>yes</block>"), Some(true));
    assert_eq!(parse_xml_block("<block>YES</block>"), Some(true));
    // Stage-1 closing tag absent (stopped on `</block>`).
    assert_eq!(parse_xml_block("<block>yes"), Some(true));
}

#[test]
fn test_parse_xml_block_no() {
    assert_eq!(parse_xml_block("<block>no</block>"), Some(false));
    assert_eq!(parse_xml_block("<block>No</block>"), Some(false));
    assert_eq!(parse_xml_block("<block>no"), Some(false));
}

#[test]
fn test_parse_xml_block_unparseable() {
    assert_eq!(parse_xml_block(""), None);
    assert_eq!(parse_xml_block("I'm not sure"), None);
    assert_eq!(parse_xml_block("<block>maybe</block>"), None);
}

#[test]
fn test_parse_xml_reason() {
    assert_eq!(
        parse_xml_reason("<reason>Destructive operation</reason>"),
        Some("Destructive operation".into())
    );
    assert_eq!(
        parse_xml_reason("<reason>  trim spaces  </reason>"),
        Some("trim spaces".into())
    );
    assert_eq!(parse_xml_reason("no reason"), None);
}

#[test]
fn test_strip_thinking_does_not_match_inner_tags() {
    // Regression: a `<block>` *inside* `<thinking>` must not be parsed.
    let text = "<thinking>I'd say <block>yes</block> for safety</thinking><block>no</block>";
    assert_eq!(parse_xml_block(text), Some(false));
}

#[test]
fn test_strip_thinking_handles_unterminated_thinking() {
    // If the response truncates inside <thinking>, the unterminated
    // segment must be ignored so we don't match its contents.
    let text = "<block>no</block>\n\n<thinking>then I would also...";
    assert_eq!(parse_xml_block(text), Some(false));
}

#[test]
fn test_build_system_prompt_with_rules_and_xml_format() {
    let rules = AutoModeRules {
        allow: vec!["git status".into(), "cargo test".into()],
        soft_deny: vec!["rm -rf".into()],
        environment: vec!["Rust project".into()],
        ..AutoModeRules::default()
    };
    let prompt = build_classifier_system_prompt(&rules);
    assert!(prompt.contains("git status"));
    assert!(prompt.contains("rm -rf"));
    assert!(prompt.contains("Rust project"));
    assert!(prompt.contains("## Output Format"));
    assert!(prompt.contains("<block>yes</block><reason>"));
    assert!(prompt.contains("<block>no</block>"));
    assert!(prompt.contains("Your ENTIRE response MUST begin with <block>"));
}

#[tokio::test]
async fn test_classify_safe_tool_skips_llm() {
    let result = classify_yolo_action(
        EMPTY_MESSAGES,
        "Read",
        &serde_json::json!({"file_path": "foo.rs"}),
        &AutoModeRules::default(),
        |_req: ClassifyRequest| async { panic!("should not call LLM for safe tool") },
        None,
    )
    .await;
    assert!(!result.should_block);
}

#[tokio::test]
async fn test_classify_stage1_allow_short_circuits() {
    let result = classify_yolo_action(
        EMPTY_MESSAGES,
        "Bash",
        &serde_json::json!({"command": "ls -la"}),
        &AutoModeRules::default(),
        |req: ClassifyRequest| async move {
            // Stage 1 must request `</block>` stop and 64-token budget.
            assert_eq!(req.stage, 1);
            assert_eq!(req.max_tokens, 64);
            assert_eq!(req.stop_sequences, Some(vec!["</block>".to_string()]));
            // Stage-1 stop_sequences truncate before the closer.
            Ok("<block>no".to_string())
        },
        None,
    )
    .await;
    assert!(!result.should_block);
    assert_eq!(result.stage, Some(1));
}

#[tokio::test]
async fn test_classify_empty_projection_fast_allows_without_llm() {
    // A non-safe tool whose projection encodes to "" declares no
    // classifier-relevant input → allow without invoking the LLM.
    // The classify_fn must never run.
    let project = |_name: &str, _input: &serde_json::Value| Some(String::new());
    let projector: Option<InputProjector> = Some(&project);
    let result = classify_yolo_action(
        EMPTY_MESSAGES,
        "Bash",
        &serde_json::json!({"command": "ls"}),
        &AutoModeRules::default(),
        |_req: ClassifyRequest| async { panic!("should not call LLM for empty projection") },
        projector,
    )
    .await;
    assert!(!result.should_block);
    assert_eq!(result.reason, "Tool declares no classifier-relevant input");
}

#[tokio::test]
async fn test_classify_none_projection_still_reaches_classifier() {
    // A projector that returns None (un-annotated) must NOT auto-allow — the
    // action reaches the classifier, which here blocks.
    let project = |_name: &str, _input: &serde_json::Value| -> Option<String> { None };
    let projector: Option<InputProjector> = Some(&project);
    let result = classify_yolo_action(
        EMPTY_MESSAGES,
        "Bash",
        &serde_json::json!({"command": "rm -rf /"}),
        &AutoModeRules::default(),
        |_req: ClassifyRequest| async { Ok("<block>yes</block>".to_string()) },
        projector,
    )
    .await;
    assert!(result.should_block);
}

#[tokio::test]
async fn test_classify_stage1_block_escalates_to_stage2() {
    let call_count = std::sync::Arc::new(std::sync::atomic::AtomicI32::new(0));
    let cc = call_count.clone();
    let result = classify_yolo_action(
        EMPTY_MESSAGES,
        "Bash",
        &serde_json::json!({"command": "rm -rf /"}),
        &AutoModeRules::default(),
        move |req: ClassifyRequest| {
            let cc = cc.clone();
            async move {
                let stage = cc.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                if stage == 1 {
                    assert_eq!(req.stage, 1);
                    Ok("<block>yes".to_string())
                } else {
                    // Stage 2 drops stop sequences and bumps budget.
                    assert_eq!(req.stage, 2);
                    assert_eq!(req.max_tokens, 4096);
                    assert_eq!(req.stop_sequences, None);
                    Ok("<block>yes</block><reason>Destructive</reason>".to_string())
                }
            }
        },
        None,
    )
    .await;
    assert!(result.should_block);
    assert_eq!(result.reason, "Destructive");
    assert_eq!(result.stage, Some(2));
}

#[tokio::test]
async fn test_classify_stage1_unparseable_escalates_to_stage2() {
    let call_count = std::sync::Arc::new(std::sync::atomic::AtomicI32::new(0));
    let cc = call_count.clone();
    let result = classify_yolo_action(
        EMPTY_MESSAGES,
        "Write",
        &serde_json::json!({"file_path": "/etc/passwd", "content": "hack"}),
        &AutoModeRules::default(),
        move |_req: ClassifyRequest| {
            let cc = cc.clone();
            async move {
                let stage = cc.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                if stage == 1 {
                    Ok("I'm not sure about this one.".to_string())
                } else {
                    Ok("<block>yes</block><reason>Writes to system file</reason>".to_string())
                }
            }
        },
        None,
    )
    .await;
    assert!(result.should_block);
    assert_eq!(result.stage, Some(2));
}

#[tokio::test]
async fn test_classify_stage2_unparseable_blocks_for_safety() {
    let result = classify_yolo_action(
        EMPTY_MESSAGES,
        "Bash",
        &serde_json::json!({"command": "rm -rf /"}),
        &AutoModeRules::default(),
        |_req: ClassifyRequest| async { Ok("garbage output".to_string()) },
        None,
    )
    .await;
    assert!(result.should_block);
    assert_eq!(result.stage, Some(2));
    assert!(result.reason.contains("unparseable"));
}

#[tokio::test]
async fn test_classify_error_defaults_to_block() {
    let result = classify_yolo_action(
        EMPTY_MESSAGES,
        "Bash",
        &serde_json::json!({"command": "rm -rf /"}),
        &AutoModeRules::default(),
        |_req: ClassifyRequest| async { Err("API error".to_string()) },
        None,
    )
    .await;
    assert!(result.should_block);
    assert_eq!(result.stage, Some(2));
}

#[tokio::test]
async fn test_classify_transport_error_marks_unavailable() {
    let result = classify_yolo_action(
        EMPTY_MESSAGES,
        "Bash",
        &serde_json::json!({"command": "rm -rf /"}),
        &AutoModeRules::default(),
        |_req: ClassifyRequest| async { Err("connection refused".to_string()) },
        None,
    )
    .await;
    assert!(result.unavailable);
    assert!(!result.transcript_too_long);
}

#[tokio::test]
async fn test_classify_prompt_too_long_marks_transcript_too_long() {
    let result = classify_yolo_action(
        EMPTY_MESSAGES,
        "Bash",
        &serde_json::json!({"command": "rm -rf /"}),
        &AutoModeRules::default(),
        |_req: ClassifyRequest| async { Err("prompt is too long: 250000 tokens".to_string()) },
        None,
    )
    .await;
    assert!(result.transcript_too_long);
    assert!(!result.unavailable);
}

#[tokio::test]
async fn test_classify_fast_mode_single_call_is_final() {
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicI32::new(0));
    let cc = calls.clone();
    let rules = AutoModeRules {
        classifier_mode: ClassifierMode::Fast,
        ..AutoModeRules::default()
    };
    let result = classify_yolo_action(
        EMPTY_MESSAGES,
        "Bash",
        &serde_json::json!({"command": "rm -rf /"}),
        &rules,
        move |req: ClassifyRequest| {
            let cc = cc.clone();
            async move {
                cc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                // Fast mode: single stage, 256-token budget, no stop sequence.
                assert_eq!(req.stage, 1);
                assert_eq!(req.max_tokens, 256);
                assert_eq!(req.stop_sequences, None);
                Ok("<block>yes</block><reason>Destructive</reason>".to_string())
            }
        },
        None,
    )
    .await;
    assert!(result.should_block);
    assert_eq!(result.reason, "Destructive");
    assert_eq!(result.stage, Some(1));
    // Fast mode never escalates — exactly one call.
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_classify_thinking_mode_skips_stage1() {
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicI32::new(0));
    let cc = calls.clone();
    let rules = AutoModeRules {
        classifier_mode: ClassifierMode::Thinking,
        ..AutoModeRules::default()
    };
    let result = classify_yolo_action(
        EMPTY_MESSAGES,
        "Bash",
        &serde_json::json!({"command": "ls"}),
        &rules,
        move |req: ClassifyRequest| {
            let cc = cc.clone();
            async move {
                cc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                // Thinking mode: stage 2 only, 4096-token budget, no stop.
                assert_eq!(req.stage, 2);
                assert_eq!(req.max_tokens, 4096);
                assert_eq!(req.stop_sequences, None);
                Ok("<block>no</block>".to_string())
            }
        },
        None,
    )
    .await;
    assert!(!result.should_block);
    assert_eq!(result.stage, Some(2));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_classify_fast_mode_block_without_reason_uses_fallback() {
    // Fast-mode block with no `<reason>` → falls back to
    // 'Blocked by fast classifier', never an empty deny message.
    let rules = AutoModeRules {
        classifier_mode: ClassifierMode::Fast,
        ..AutoModeRules::default()
    };
    let result = classify_yolo_action(
        EMPTY_MESSAGES,
        "Bash",
        &serde_json::json!({"command": "rm -rf /"}),
        &rules,
        |_req: ClassifyRequest| async { Ok("<block>yes</block>".to_string()) },
        None,
    )
    .await;
    assert!(result.should_block);
    assert_eq!(result.reason, "Blocked by fast classifier");
    assert_eq!(result.stage, Some(1));
}

#[tokio::test]
async fn test_classify_stage2_block_without_reason_uses_fallback() {
    // Two-stage block reaching stage 2 with no `<reason>` → falls back to
    // 'No reason provided', never an empty deny message.
    let result = classify_yolo_action(
        EMPTY_MESSAGES,
        "Bash",
        &serde_json::json!({"command": "rm -rf /"}),
        &AutoModeRules::default(),
        |req: ClassifyRequest| async move {
            if req.stage == 1 {
                Ok("<block>yes".to_string())
            } else {
                Ok("<block>yes</block>".to_string())
            }
        },
        None,
    )
    .await;
    assert!(result.should_block);
    assert_eq!(result.reason, "No reason provided");
    assert_eq!(result.stage, Some(2));
}

#[test]
fn test_format_action_uses_projector_when_some() {
    let project = |_name: &str, _input: &serde_json::Value| Some("PROJECTED".to_string());
    let projector: InputProjector<'_> = &project;
    let action = format_action_for_classifier(
        "Bash",
        &serde_json::json!({"command": "rm -rf /"}),
        Some(projector),
    );
    assert!(action.contains("PROJECTED"));
    // The raw input is hidden behind the curated projection.
    assert!(!action.contains("rm -rf"));
}

#[test]
fn test_format_action_projector_none_falls_back_to_raw() {
    let project = |_name: &str, _input: &serde_json::Value| None;
    let projector: InputProjector<'_> = &project;
    let action = format_action_for_classifier(
        "Bash",
        &serde_json::json!({"command": "rm -rf /"}),
        Some(projector),
    );
    // No projection → raw JSON fallback (the action still reaches the gate).
    assert!(action.contains("rm -rf"));
}

#[test]
fn test_truncate_multibyte_boundary_no_panic() {
    // 499 ASCII bytes + a 3-byte char straddling byte 500 must not panic.
    let s = format!("{}中", "a".repeat(499));
    let out = truncate(&s, 500);
    assert!(out.ends_with("..."));
    // The multibyte char was dropped at the boundary, not split.
    assert!(out.starts_with(&"a".repeat(499)));
    assert!(!out.contains('中'));
}

#[test]
fn test_truncate_short_string_unchanged() {
    assert_eq!(truncate("hello", 100), "hello");
}

#[test]
fn test_format_transcript_is_chronological() {
    let entries = vec![
        TranscriptEntry {
            role: TranscriptRole::User,
            content: vec![TranscriptBlock::Text("first-marker".into())],
        },
        TranscriptEntry {
            role: TranscriptRole::Assistant,
            content: vec![TranscriptBlock::ToolCall {
                tool_name: "Bash".into(),
                input_summary: "ls".into(),
            }],
        },
        TranscriptEntry {
            role: TranscriptRole::User,
            content: vec![TranscriptBlock::Text("second-marker".into())],
        },
    ];
    let out = format_transcript(&entries);
    let first = out.find("first-marker").expect("first present");
    let second = out.find("second-marker").expect("second present");
    assert!(first < second, "transcript must be chronological:\n{out}");
}
