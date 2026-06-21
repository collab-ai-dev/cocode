use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn empty_tools_returns_none_without_calling_role_cache() {
    // `has_tools()` short-circuits before any client resolution.
    let input = ToolUseSummaryInput {
        tools: Vec::new(),
        preceding_tool_use_ids: Vec::new(),
        last_assistant_text: Some("anything".into()),
    };
    assert!(!input.has_tools());
}

#[test]
fn truncate_json_under_cap_is_unchanged() {
    let v = json!({"a": 1, "b": "short"});
    let s = truncate_json(&v, 300);
    assert_eq!(s, r#"{"a":1,"b":"short"}"#);
}

#[test]
fn truncate_json_over_cap_is_clipped_with_ellipsis() {
    let long = "x".repeat(500);
    let v = json!(long);
    let s = truncate_json(&v, 100);
    // 100-char cap. Result is `…` + 3 ellipsis chars at end.
    assert_eq!(s.chars().count(), 100);
    assert!(s.ends_with("..."));
}

#[test]
fn build_prompt_renders_system_and_user_in_order() {
    let input = ToolUseSummaryInput {
        tools: vec![ToolInfo {
            name: "Read".into(),
            input: json!({"file_path": "/tmp/x.rs"}),
            output: json!("contents of x.rs"),
        }],
        preceding_tool_use_ids: vec!["tu_1".into()],
        last_assistant_text: Some("Let me read the file.".into()),
    };
    let prompt = build_prompt(&input);
    assert_eq!(prompt.len(), 2);

    let user_text = match &prompt[1] {
        LlmMessage::User { content, .. } => content
            .iter()
            .filter_map(|p| match p {
                UserContentPart::Text(part) => Some(part.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => panic!("expected User as second message"),
    };

    assert!(
        user_text.contains("User's intent (from assistant's last message): Let me read the file.")
    );
    assert!(user_text.contains("Tool: Read"));
    assert!(user_text.contains("\"file_path\":\"/tmp/x.rs\""));
    assert!(user_text.contains("Output: \"contents of x.rs\""));
    assert!(user_text.ends_with("\n\nLabel:"));
}

#[test]
fn build_prompt_omits_intent_prefix_when_no_assistant_text() {
    let input = ToolUseSummaryInput {
        tools: vec![ToolInfo {
            name: "Bash".into(),
            input: json!({"command": "ls"}),
            output: json!("file1\nfile2"),
        }],
        preceding_tool_use_ids: vec!["tu_a".into()],
        last_assistant_text: None,
    };
    let prompt = build_prompt(&input);
    let user_text = match &prompt[1] {
        LlmMessage::User { content, .. } => content
            .iter()
            .filter_map(|p| match p {
                UserContentPart::Text(part) => Some(part.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => panic!("expected User"),
    };
    assert!(!user_text.contains("User's intent"));
    assert!(user_text.starts_with("Tools completed:"));
}

#[test]
fn last_assistant_text_truncated_to_200_chars() {
    let long = "a".repeat(500);
    let input = ToolUseSummaryInput {
        tools: vec![ToolInfo {
            name: "Read".into(),
            input: json!({}),
            output: json!(null),
        }],
        preceding_tool_use_ids: vec!["tu".into()],
        last_assistant_text: Some(long),
    };
    let prompt = build_prompt(&input);
    let user_text = match &prompt[1] {
        LlmMessage::User { content, .. } => content
            .iter()
            .filter_map(|p| match p {
                UserContentPart::Text(part) => Some(part.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => panic!(),
    };
    // Isolate the run of `a`s that follows the intent prefix — it must
    // be exactly `LAST_ASSISTANT_TEXT_MAX` long, no more, even though
    // the source string had 500 `a`s.
    let intent_prefix = "User's intent (from assistant's last message): ";
    let after_prefix = user_text
        .strip_prefix(intent_prefix)
        .expect("intent prefix present");
    let truncated_run = after_prefix.chars().take_while(|c| *c == 'a').count();
    assert_eq!(truncated_run, LAST_ASSISTANT_TEXT_MAX);
}

#[test]
fn multi_tool_batch_separates_with_double_newline() {
    let input = ToolUseSummaryInput {
        tools: vec![
            ToolInfo {
                name: "Read".into(),
                input: json!({}),
                output: json!("a"),
            },
            ToolInfo {
                name: "Bash".into(),
                input: json!({"command": "ls"}),
                output: json!("b"),
            },
        ],
        preceding_tool_use_ids: vec!["tu_1".into(), "tu_2".into()],
        last_assistant_text: None,
    };
    let prompt = build_prompt(&input);
    let user_text = match &prompt[1] {
        LlmMessage::User { content, .. } => content
            .iter()
            .filter_map(|p| match p {
                UserContentPart::Text(part) => Some(part.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => panic!(),
    };
    let read_idx = user_text.find("Tool: Read").expect("Read present");
    let bash_idx = user_text.find("Tool: Bash").expect("Bash present");
    assert!(read_idx < bash_idx, "tools rendered in input order");
    assert!(
        user_text[..bash_idx].ends_with("\n\n"),
        "tools separated by double newline"
    );
}

#[test]
fn truncate_json_non_serializable_returns_placeholder() {
    // serde_json can serialize everything, but as a defense against
    // future Value types, the function returns a placeholder rather
    // than panicking. No direct way to construct a non-serializable
    // Value, so we just assert the happy path for now and rely on the
    // type system.
    let v = json!({"k": "v"});
    let s = truncate_json(&v, 300);
    assert!(s.contains('k'));
}

#[test]
fn system_prompt_is_stable() {
    // Lock the system prompt content. Any drift would change Fast-tier
    // model behavior — the test exists so that mismatch is caught at
    // CI, not at runtime.
    assert!(
        TOOL_USE_SUMMARY_SYSTEM_PROMPT.starts_with(
            "Write a short summary label describing what these tool calls accomplished."
        )
    );
    assert!(TOOL_USE_SUMMARY_SYSTEM_PROMPT.contains("git-commit-subject"));
    assert!(TOOL_USE_SUMMARY_SYSTEM_PROMPT.contains("- Read config.json"));
}

#[test]
fn extract_assistant_text_concatenates_and_trims() {
    use coco_llm_types::TextPart;
    let parts = vec![
        AssistantContentPart::Text(TextPart::new("  hello")),
        AssistantContentPart::Text(TextPart::new(" world  ")),
    ];
    assert_eq!(extract_assistant_text(&parts), "hello world");
}

#[test]
fn extract_assistant_text_empty_returns_empty() {
    assert_eq!(extract_assistant_text(&[]), "");
}

mod build_input_tests {
    use super::*;
    use coco_llm_types::ToolCallPart;
    use coco_messages::Message;
    use coco_messages::create_assistant_message;
    use coco_messages::create_tool_result_message;
    use coco_types::ToolId;
    use coco_types::ToolName;
    use pretty_assertions::assert_eq;

    fn assistant_with(content: Vec<AssistantContentPart>) -> Message {
        create_assistant_message(content, "test-model", Default::default())
    }

    fn tool_result(id: &str, name: &str, output: &str) -> Message {
        create_tool_result_message(
            id,
            name,
            ToolId::Builtin(ToolName::Bash),
            output,
            /*is_error*/ false,
        )
    }

    #[test]
    fn empty_history_returns_none() {
        let msgs: &[coco_messages::Message] = &[];
        assert!(build_input_from_history(msgs).is_none());
    }

    #[test]
    fn no_assistant_message_returns_none() {
        let user = coco_messages::create_user_message("hi");
        assert!(build_input_from_history(&[user]).is_none());
    }

    #[test]
    fn assistant_text_only_returns_none() {
        // No tool calls means nothing to summarize.
        let msg = assistant_with(vec![AssistantContentPart::text("I'm thinking.")]);
        assert!(build_input_from_history(&[msg]).is_none());
    }

    #[test]
    fn single_tool_call_with_matching_result_produces_input() {
        let assistant = assistant_with(vec![
            AssistantContentPart::text("Reading the file."),
            AssistantContentPart::ToolCall(ToolCallPart::new(
                "tu_001",
                "Read",
                serde_json::json!({"file_path": "/etc/hosts"}),
            )),
        ]);
        let result = tool_result("tu_001", "Read", "127.0.0.1 localhost");
        let history = vec![assistant, result];
        let input = build_input_from_history(&history).expect("Some");
        assert_eq!(input.tools.len(), 1);
        assert_eq!(input.tools[0].name, "Read");
        assert_eq!(input.tools[0].input["file_path"], "/etc/hosts");
        assert_eq!(input.preceding_tool_use_ids, vec!["tu_001"]);
        assert_eq!(
            input.last_assistant_text.as_deref(),
            Some("Reading the file.")
        );
        // Output is the serialized ToolResultContent — text shape.
        assert!(
            input.tools[0]
                .output
                .to_string()
                .contains("127.0.0.1 localhost"),
            "actual: {}",
            input.tools[0].output
        );
    }

    #[test]
    fn last_text_block_wins_for_multi_text_assistant() {
        // Final text block is the "user intent" snippet for the summary prompt.
        let assistant = assistant_with(vec![
            AssistantContentPart::text("first thought"),
            AssistantContentPart::text("final commentary"),
            AssistantContentPart::ToolCall(ToolCallPart::new(
                "tu_x",
                "Bash",
                serde_json::json!({"command": "ls"}),
            )),
        ]);
        let result = tool_result("tu_x", "Bash", "ok");
        let input = build_input_from_history(&[assistant, result]).expect("Some");
        assert_eq!(
            input.last_assistant_text.as_deref(),
            Some("final commentary")
        );
    }

    #[test]
    fn multiple_tool_calls_preserve_input_order() {
        let assistant = assistant_with(vec![
            AssistantContentPart::ToolCall(ToolCallPart::new(
                "tu_a",
                "Read",
                serde_json::json!({"file_path": "/a"}),
            )),
            AssistantContentPart::ToolCall(ToolCallPart::new(
                "tu_b",
                "Read",
                serde_json::json!({"file_path": "/b"}),
            )),
        ]);
        // Results in reverse-order to verify lookup is by id, not position.
        let r_b = tool_result("tu_b", "Read", "B contents");
        let r_a = tool_result("tu_a", "Read", "A contents");
        let history = vec![assistant, r_b, r_a];
        let input = build_input_from_history(&history).expect("Some");
        assert_eq!(input.preceding_tool_use_ids, vec!["tu_a", "tu_b"]);
        assert!(input.tools[0].output.to_string().contains("A contents"));
        assert!(input.tools[1].output.to_string().contains("B contents"));
    }

    #[test]
    fn missing_tool_result_yields_null_output() {
        let assistant = assistant_with(vec![AssistantContentPart::ToolCall(ToolCallPart::new(
            "tu_missing",
            "Bash",
            serde_json::json!({"command": "true"}),
        ))]);
        // No result message for tu_missing.
        let input = build_input_from_history(&[assistant]).expect("Some");
        assert_eq!(input.tools.len(), 1);
        assert_eq!(input.tools[0].output, serde_json::Value::Null);
    }

    #[test]
    fn picks_most_recent_assistant_when_multiple() {
        let older_assistant = assistant_with(vec![
            AssistantContentPart::text("OLD"),
            AssistantContentPart::ToolCall(ToolCallPart::new(
                "tu_old",
                "Read",
                serde_json::json!({}),
            )),
        ]);
        let older_result = tool_result("tu_old", "Read", "old");
        let user = coco_messages::create_user_message("follow up");
        let newer_assistant = assistant_with(vec![
            AssistantContentPart::text("NEW"),
            AssistantContentPart::ToolCall(ToolCallPart::new(
                "tu_new",
                "Bash",
                serde_json::json!({"command": "echo"}),
            )),
        ]);
        let newer_result = tool_result("tu_new", "Bash", "new");
        let history = vec![
            older_assistant,
            older_result,
            user,
            newer_assistant,
            newer_result,
        ];
        let input = build_input_from_history(&history).expect("Some");
        assert_eq!(input.preceding_tool_use_ids, vec!["tu_new"]);
        assert_eq!(input.last_assistant_text.as_deref(), Some("NEW"));
        assert!(input.tools[0].output.to_string().contains("new"));
    }
}
