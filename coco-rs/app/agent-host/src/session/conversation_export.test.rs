use std::sync::Arc;

use coco_messages::AssistantContent;
use coco_messages::Message;
use coco_messages::create_assistant_message;
use coco_messages::create_user_message;

use super::ExportFormat;

#[test]
fn renders_user_and_assistant_roles_as_text() {
    let msgs = vec![
        Arc::new(create_user_message("hi there")),
        Arc::new(create_assistant_message(
            vec![AssistantContent::text("hello!")],
            "test-model",
            Default::default(),
        )),
    ];
    let out = ExportFormat::Text.render(&msgs);
    assert!(out.contains("User:\nhi there"), "got: {out}");
    assert!(out.contains("Assistant:\nhello!"), "got: {out}");
}

#[test]
fn markdown_includes_assistant_tool_calls() {
    use coco_llm_types::ToolCallPart;
    let assistant = Arc::new(create_assistant_message(
        vec![
            AssistantContent::text("running it"),
            AssistantContent::ToolCall(ToolCallPart::new(
                "c1",
                "Bash",
                serde_json::json!({ "command": "ls" }),
            )),
        ],
        "test-model",
        Default::default(),
    ));
    let out = ExportFormat::Markdown.render(&[assistant]);
    assert!(out.contains("## Assistant"), "got: {out}");
    assert!(
        out.contains("Tool call · Bash"),
        "tool call rendered: {out}"
    );
}

#[test]
fn format_inferred_from_filename_extension() {
    assert!(matches!(
        ExportFormat::from_filename("notes.md"),
        ExportFormat::Markdown
    ));
    assert!(matches!(
        ExportFormat::from_filename("data.json"),
        ExportFormat::Json
    ));
    assert!(matches!(
        ExportFormat::from_filename("plain.txt"),
        ExportFormat::Text
    ));
    assert!(matches!(
        ExportFormat::from_filename("noext"),
        ExportFormat::Text
    ));
}

#[test]
fn empty_conversation_renders_empty_text() {
    let msgs: Vec<Arc<Message>> = vec![];
    assert_eq!(ExportFormat::Text.render(&msgs), "");
}
