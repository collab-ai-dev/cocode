use super::AskUserQuestionTool;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::DynTool;
use coco_tool_runtime::PromptOptions;
use coco_tool_runtime::ToolUseContext;
use pretty_assertions::assert_eq;
use serde_json::json;

fn minimal_input() -> serde_json::Value {
    json!({
        "questions": [{
            "question": "Pick one?",
            "header": "Pick",
            "options": [
                {"label": "A", "description": "first"},
                {"label": "B", "description": "second"}
            ]
        }]
    })
}

#[test]
fn name_matches_tool_name_enum() {
    let t: &dyn DynTool = &AskUserQuestionTool;
    assert_eq!(t.name(), coco_types::ToolName::AskUserQuestion.as_str());
}

#[test]
fn description_has_expected_phrase() {
    let t: &dyn DynTool = &AskUserQuestionTool;
    let d = t.description(&minimal_input(), &DescriptionOptions::default());
    assert!(
        d.contains("multiple choice questions"),
        "description should mention multiple choice questions: {d}"
    );
}

#[tokio::test]
async fn prompt_includes_plan_mode_and_preview_guidance() {
    let t: &dyn DynTool = &AskUserQuestionTool;
    let p = t.prompt(&PromptOptions::default()).await;
    // Plan-mode guidance must call out ExitPlanMode by name — sourced
    // from `ToolName::ExitPlanMode.as_str()` so the test will fail
    // loudly if the enum variant is renamed without updating callers.
    let exit = coco_types::ToolName::ExitPlanMode.as_str();
    assert!(p.contains(exit), "prompt missing {exit} reference: {p}");
    // The IMPORTANT block must stay intact — the model otherwise asks
    // meta-questions about an invisible plan.
    assert!(
        p.contains("Do not reference \"the plan\""),
        "prompt missing IMPORTANT block on plan-reference avoidance"
    );
    // Preview-feature section must be present.
    assert!(
        p.contains("Preview feature:"),
        "prompt missing preview feature section"
    );
    assert!(
        p.contains("multiSelect"),
        "prompt missing multiSelect guidance"
    );
    // "Other" option note.
    assert!(p.contains("\"Other\""), "prompt missing Other option note");
    // Recommendation rule.
    assert!(
        p.contains("(Recommended)"),
        "prompt missing recommendation guidance"
    );
}

#[test]
fn input_schema_has_questions_array() {
    let t: &dyn DynTool = &AskUserQuestionTool;
    let schema = t.runtime_validation_schema().as_value();
    let top_required = schema["required"]
        .as_array()
        .expect("top-level required missing");
    assert!(top_required.iter().any(|v| v == "questions"));
    let questions = schema["properties"]
        .get("questions")
        .expect("questions property missing");
    assert_eq!(questions["type"], "array");
    assert_eq!(questions["minItems"], 1);
    assert_eq!(questions["maxItems"], 4);
    assert!(
        questions["description"]
            .as_str()
            .unwrap_or_default()
            .contains("1-4"),
        "{questions}"
    );
    let items = &questions["items"];
    let required = items["required"]
        .as_array()
        .expect("items.required missing");
    assert!(required.iter().any(|v| v == "question"));
    assert!(required.iter().any(|v| v == "header"));
    assert!(required.iter().any(|v| v == "options"));
    let options = &items["properties"]["options"];
    assert_eq!(options["minItems"], 2);
    assert_eq!(options["maxItems"], 4);
    assert_eq!(
        items["properties"]["multiSelect"]["default"],
        serde_json::Value::Bool(false)
    );
}

#[test]
fn input_schema_rejects_legacy_message_field() {
    let t: &dyn DynTool = &AskUserQuestionTool;
    let err = t
        .runtime_validation_schema()
        .validate(&json!({"message": "What do you need?"}))
        .expect_err("message must not be accepted as a questions alias");
    assert!(
        err.iter().any(|issue| matches!(
            issue,
            coco_tool_runtime::schema::SchemaIssue::UnexpectedField { field, .. }
                if field == "message"
        ) || matches!(
            issue,
            coco_tool_runtime::schema::SchemaIssue::MissingRequired { field, .. }
                if field == "questions"
        )),
        "expected schema error to mention message or questions: {err:?}"
    );
}

#[test]
fn input_schema_rejects_more_than_four_questions_or_options() {
    let t: &dyn DynTool = &AskUserQuestionTool;
    let question = json!({
        "question": "Pick?",
        "header": "Pick",
        "options": [
            {"label": "A", "description": "a"},
            {"label": "B", "description": "b"}
        ]
    });
    let too_many_questions = json!({
        "questions": [
            question.clone(),
            question.clone(),
            question.clone(),
            question.clone(),
            question
        ]
    });
    assert!(
        t.runtime_validation_schema()
            .validate(&too_many_questions)
            .is_err(),
        "questions.maxItems must be enforced"
    );

    let too_many_options = json!({
        "questions": [{
            "question": "Pick?",
            "header": "Pick",
            "options": [
                {"label": "A", "description": "a"},
                {"label": "B", "description": "b"},
                {"label": "C", "description": "c"},
                {"label": "D", "description": "d"},
                {"label": "E", "description": "e"}
            ]
        }]
    });
    assert!(
        t.runtime_validation_schema()
            .validate(&too_many_options)
            .is_err(),
        "options.maxItems must be enforced"
    );
}

/// Field descriptions must carry expected guidance. Weak models (e.g.
/// deepseek-v4-flash) rely on these to know `question` is required — stripped
/// descriptions caused real `questions[0].question is missing` failures.
#[test]
fn field_descriptions_are_aligned() {
    let t: &dyn DynTool = &AskUserQuestionTool;
    let schema = t.runtime_validation_schema().as_value();
    let props = &schema["properties"]["questions"]["items"]["properties"];
    let desc = |v: &serde_json::Value| v["description"].as_str().unwrap_or_default().to_string();

    assert!(
        desc(&props["question"]).contains("end with a question mark"),
        "question description lost expected guidance"
    );
    // Chip width must read 12, not 20.
    let header = desc(&props["header"]);
    assert!(
        header.contains("max 12 chars"),
        "header chip width drifted: {header}"
    );
    assert!(
        header.contains("Auth method"),
        "header lost expected examples: {header}"
    );
    assert!(
        desc(&props["options"]).contains("There should be no 'Other' option"),
        "options description lost the auto-Other guidance"
    );
    assert!(
        desc(&props["multiSelect"]).contains("mutually exclusive"),
        "multiSelect description lost expected guidance"
    );
}

#[test]
fn tool_requires_user_interaction_and_is_concurrency_safe() {
    let t: &dyn DynTool = &AskUserQuestionTool;
    assert!(t.requires_user_interaction());
    assert!(t.is_concurrency_safe(&minimal_input()));
}

#[test]
fn validate_input_rejects_duplicate_question_text() {
    let t: &dyn DynTool = &AskUserQuestionTool;
    let ctx = ToolUseContext::test_default();
    let input = json!({
        "questions": [
            {
                "question": "Pick one?",
                "header": "First",
                "options": [
                    {"label": "A", "description": "a"},
                    {"label": "B", "description": "b"}
                ]
            },
            {
                "question": "Pick one?",
                "header": "Second",
                "options": [
                    {"label": "C", "description": "c"},
                    {"label": "D", "description": "d"}
                ]
            }
        ]
    });
    let result = t.validate_input(&input, &ctx);
    assert!(
        !result.is_valid(),
        "duplicate question text must be rejected"
    );
}

#[test]
fn validate_input_rejects_duplicate_option_labels_per_question() {
    let t: &dyn DynTool = &AskUserQuestionTool;
    let ctx = ToolUseContext::test_default();
    let input = json!({
        "questions": [{
            "question": "Pick one?",
            "header": "Pick",
            "options": [
                {"label": "A", "description": "a"},
                {"label": "A", "description": "duplicate"}
            ]
        }]
    });
    let result = t.validate_input(&input, &ctx);
    assert!(
        !result.is_valid(),
        "duplicate option labels must be rejected"
    );
}

#[tokio::test]
async fn execute_echoes_questions_payload() {
    let t: &dyn DynTool = &AskUserQuestionTool;
    let ctx = ToolUseContext::test_default();
    let input = json!({
        "questions": [{
            "question": "Pick one",
            "header": "q1",
            "options": [
                {"label": "A", "description": "first"},
                {"label": "B", "description": "second"}
            ]
        }]
    });
    let out = t.execute(input.clone(), &ctx).await.expect("execute ok");
    assert_eq!(out.data["questions"], input["questions"]);
}

#[tokio::test]
async fn execute_emits_structured_display_data_for_styled_cell() {
    use coco_types::ToolDisplayData;
    let t: &dyn DynTool = &AskUserQuestionTool;
    let ctx = ToolUseContext::test_default();
    let input = json!({
        "questions": [
            {"question": "Pick a library?", "header": "Lib", "options": [{"label": "date-fns", "description": ""}]},
            {"question": "Which features?", "header": "Feat", "options": [{"label": "i18n", "description": ""}]},
            {"question": "Anything else?", "header": "More", "options": [{"label": "Other", "description": ""}]}
        ],
        // Note: answers map order differs from the questions array — the display
        // cell must follow the questions order.
        "answers": {
            "Which features?": "i18n, timezones",
            "Pick a library?": "date-fns",
            "Anything else?": "a custom answer"
        },
        "annotations": {"Anything else?": {"notes": "extra context"}}
    });
    let out = t.execute(input, &ctx).await.expect("execute ok");
    let Some(ToolDisplayData::AskUserQuestionResult(result)) = out.display_data else {
        panic!("expected AskUserQuestionResult display data");
    };
    assert_eq!(result.questions.len(), 3);
    // Order follows the questions array, not the answers map.
    assert_eq!(result.questions[0].question, "Pick a library?");
    assert_eq!(result.questions[0].answers, vec!["date-fns".to_string()]);
    // Multi-select answers are ", "-joined by build_answer_payload → split back.
    assert_eq!(
        result.questions[1].answers,
        vec!["i18n".to_string(), "timezones".to_string()]
    );
    assert_eq!(result.questions[2].note, Some("extra context".to_string()));
}

#[tokio::test]
async fn execute_without_answers_has_no_display_data() {
    let t: &dyn DynTool = &AskUserQuestionTool;
    let ctx = ToolUseContext::test_default();
    let input = json!({
        "questions": [{"question": "Q?", "header": "h", "options": [{"label": "A", "description": ""}]}]
    });
    let out = t.execute(input, &ctx).await.expect("execute ok");
    assert!(
        out.display_data.is_none(),
        "no answers spliced ⇒ render falls back to the prose"
    );
}

// ── render_for_model — answer envelope rendering ─────────────────────

mod render_tests {
    use super::AskUserQuestionTool;
    use coco_tool_runtime::DynTool;

    use coco_tool_runtime::ToolResultContentPart;
    use serde_json::json;

    fn text_of(parts: &[ToolResultContentPart]) -> &str {
        match &parts[0] {
            ToolResultContentPart::Text { text, .. } => text.as_str(),
            _ => panic!("expected Text part"),
        }
    }

    #[test]
    fn formats_single_answer_with_trailing_continuation_clause() {
        let data = json!({
            "answers": {"What's your name?": "Alice"},
        });
        let parts = <AskUserQuestionTool as DynTool>::render_for_model(&AskUserQuestionTool, &data);
        assert_eq!(
            text_of(&parts),
            "User has answered your questions: \"What's your name?\"=\"Alice\". You can now continue with the user's answers in mind."
        );
    }

    #[test]
    fn joins_multiple_answers_with_comma_space() {
        // Order is not guaranteed (HashMap-backed) so check both
        // entries are present and the boilerplate wraps them.
        let data = json!({
            "answers": {"Q1": "A1", "Q2": "A2"},
        });
        let parts = <AskUserQuestionTool as DynTool>::render_for_model(&AskUserQuestionTool, &data);
        let text = text_of(&parts);
        assert!(text.starts_with("User has answered your questions: "));
        assert!(text.contains("\"Q1\"=\"A1\""));
        assert!(text.contains("\"Q2\"=\"A2\""));
        assert!(text.ends_with(". You can now continue with the user's answers in mind."));
    }

    #[test]
    fn appends_preview_and_notes_when_annotation_present() {
        // `selected preview:\n...` and `user notes: ...` join with single space.
        let data = json!({
            "answers": {"Pick a layout": "two-column"},
            "annotations": {
                "Pick a layout": {
                    "preview": "+----+----+",
                    "notes": "user prefers compact"
                }
            }
        });
        let parts = <AskUserQuestionTool as DynTool>::render_for_model(&AskUserQuestionTool, &data);
        let text = text_of(&parts);
        assert!(text.contains("\"Pick a layout\"=\"two-column\""));
        assert!(text.contains("selected preview:\n+----+----+"));
        assert!(text.contains("user notes: user prefers compact"));
    }

    #[test]
    fn preview_only_skips_notes_clause() {
        let data = json!({
            "answers": {"Q": "A"},
            "annotations": {"Q": {"preview": "snip"}}
        });
        let parts = <AskUserQuestionTool as DynTool>::render_for_model(&AskUserQuestionTool, &data);
        let text = text_of(&parts);
        assert!(text.contains("selected preview:\nsnip"));
        assert!(!text.contains("user notes:"));
    }

    #[test]
    fn missing_answers_falls_through_to_json_envelope() {
        // Pre-splicer envelope (questions only) — the TUI hasn't
        // collected answers yet. Defensive path emits the data as
        // JSON-or-string text so nothing leaks unrendered.
        let data = json!({
            "questions": [{"question": "Pick", "options": [], "header": "h"}]
        });
        let parts = <AskUserQuestionTool as DynTool>::render_for_model(&AskUserQuestionTool, &data);
        let text = text_of(&parts);
        assert!(
            text.starts_with('{') || text.starts_with('"'),
            "expected JSON or string fallback, got: {text}"
        );
    }

    #[test]
    fn empty_answers_object_falls_through() {
        let data = json!({"answers": {}});
        let parts = <AskUserQuestionTool as DynTool>::render_for_model(&AskUserQuestionTool, &data);
        let text = text_of(&parts);
        // The defensive `render_text_or_json` JSON-stringifies the
        // whole envelope when it can't extract a flat string.
        assert!(text.contains("\"answers\""), "got: {text}");
    }
}

#[tokio::test]
async fn check_permissions_always_asks_to_drive_question_overlay() {
    // AskUserQuestion is read-only, so without an Ask override the evaluator
    // auto-allows it and execute() echoes raw JSON. Returning Ask is what routes
    // the call through the permission bridge into the interactive Question overlay.
    let t: &dyn DynTool = &AskUserQuestionTool;
    let ctx = ToolUseContext::test_default();
    let result = t.check_permissions(&json!({ "questions": [] }), &ctx).await;
    assert!(
        matches!(result, coco_types::ToolCheckResult::Ask { .. }),
        "expected Ask to trigger the question overlay, got {result:?}"
    );
}
