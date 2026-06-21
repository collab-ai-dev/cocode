use std::path::PathBuf;

use coco_context::file_read_state::FileReadEntry;
use pretty_assertions::assert_eq;
use uuid::Uuid;

use coco_llm_types::ToolCallPart;

use super::*;

fn make_entry(content: &str, mtime: i64) -> FileReadEntry {
    FileReadEntry::full_real(content.to_string(), mtime)
}

fn make_assistant_with_read_tool_call(tool_call_id: &str, file_path: &str) -> Message {
    let read_name = ToolName::Read.as_str();
    Message::Assistant(coco_messages::AssistantMessage {
        message: LlmMessage::assistant(vec![AssistantContentPart::ToolCall(ToolCallPart::new(
            tool_call_id,
            read_name,
            serde_json::json!({"file_path": file_path}),
        ))]),
        uuid: Uuid::new_v4(),
        model: String::new(),
        stop_reason: None,
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    })
}

fn make_tool_result(tool_use_id: &str, text: &str) -> Message {
    Message::ToolResult(coco_messages::ToolResultMessage {
        uuid: Uuid::new_v4(),
        source_assistant_uuid: None,
        display_data: None,
        message: LlmMessage::Tool {
            content: vec![coco_messages::ToolContent::ToolResult(
                coco_llm_types::ToolResultPart {
                    tool_call_id: tool_use_id.to_string(),
                    tool_name: String::new(),
                    output: coco_llm_types::ToolResultContent::text(text),
                    is_error: false,
                    provider_metadata: None,
                },
            )],
            provider_options: None,
        },
        tool_use_id: tool_use_id.to_string(),
        tool_id: coco_types::ToolId::Builtin(ToolName::Read),
        is_error: false,
    })
}

#[test]
fn test_collect_read_tool_file_paths_basic() {
    let messages = vec![
        make_assistant_with_read_tool_call("call_1", "/src/main.rs"),
        make_tool_result("call_1", "fn main() {}"),
    ];
    let paths = collect_read_tool_file_paths(&messages);
    assert_eq!(paths.len(), 1);
    assert!(paths.contains(&PathBuf::from("/src/main.rs")));
}

#[test]
fn test_collect_read_tool_file_paths_excludes_stubs() {
    let messages = vec![
        make_assistant_with_read_tool_call("call_1", "/src/main.rs"),
        make_tool_result(
            "call_1",
            "File unchanged since last read. The content from the earlier Read...",
        ),
    ];
    let paths = collect_read_tool_file_paths(&messages);
    assert!(paths.is_empty(), "stub tool results should be excluded");
}

#[test]
fn test_should_exclude_plan_file() {
    let cwd = PathBuf::from("/project");
    let plan = PathBuf::from("/project/.claude/plan.md");
    assert!(should_exclude_from_restore(&plan, &cwd, Some(&plan)));
    assert!(!should_exclude_from_restore(
        &PathBuf::from("/project/src/main.rs"),
        &cwd,
        Some(&plan)
    ));
}

#[test]
fn test_should_exclude_claude_md() {
    let cwd = PathBuf::from("/project");
    assert!(should_exclude_from_restore(
        &PathBuf::from("/project/CLAUDE.md"),
        &cwd,
        None
    ));
    assert!(should_exclude_from_restore(
        &PathBuf::from("/project/CLAUDE.local.md"),
        &cwd,
        None
    ));
    assert!(!should_exclude_from_restore(
        &PathBuf::from("/project/src/lib.rs"),
        &cwd,
        None
    ));
}

#[test]
fn test_create_post_compact_file_attachments_basic() {
    let dir = tempfile::tempdir().expect("tmpdir");
    let file_a = dir.path().join("a.rs");
    let file_b = dir.path().join("b.rs");
    std::fs::write(&file_a, "fn a() {}").expect("write");
    std::fs::write(&file_b, "fn b() {}").expect("write");

    let snapshot = vec![
        (file_a, make_entry("fn a() {}", 1)),
        (file_b, make_entry("fn b() {}", 2)),
    ];

    let atts = create_post_compact_file_attachments::<Message>(
        &snapshot,
        &[],
        dir.path(),
        /*plan*/ None,
    );
    assert_eq!(atts.len(), 2, "should restore both files");
    assert_eq!(
        atts[0].kind,
        coco_types::AttachmentKind::CompactFileReference
    );
    let Some(coco_messages::AttachmentExtras::CompactFileReference(payload)) =
        atts[0].extras.as_ref()
    else {
        panic!("compact file reference should preserve typed display payload");
    };
    assert_eq!(payload.display_path, "b.rs");
    // Most recent (b.rs, last in snapshot) should come first in result
    let text0 = format!("{:?}", atts[0].as_api_message().expect("api body"));
    assert!(
        text0.contains("b.rs"),
        "most recent file should be first: {text0}"
    );
}

#[test]
fn test_create_post_compact_file_attachments_skips_preserved() {
    let dir = tempfile::tempdir().expect("tmpdir");
    let file_a = dir.path().join("a.rs");
    std::fs::write(&file_a, "fn a() {}").expect("write");

    let snapshot = vec![(file_a.clone(), make_entry("fn a() {}", 1))];

    // Preserved messages contain a Read of a.rs
    let preserved = vec![
        make_assistant_with_read_tool_call("c1", file_a.to_str().expect("path")),
        make_tool_result("c1", "fn a() {}"),
    ];

    let atts =
        create_post_compact_file_attachments(&snapshot, &preserved, dir.path(), /*plan*/ None);
    assert!(
        atts.is_empty(),
        "should skip file already in preserved messages"
    );
}

#[test]
fn test_create_post_compact_file_attachments_respects_max_files() {
    let dir = tempfile::tempdir().expect("tmpdir");
    let mut snapshot = Vec::new();
    for i in 0..10 {
        let file = dir.path().join(format!("file{i}.rs"));
        std::fs::write(&file, format!("fn f{i}() {{}}")).expect("write");
        snapshot.push((file, make_entry(&format!("fn f{i}() {{}}"), i)));
    }

    let atts = create_post_compact_file_attachments::<Message>(
        &snapshot,
        &[],
        dir.path(),
        /*plan*/ None,
    );
    assert_eq!(
        atts.len(),
        POST_COMPACT_MAX_FILES_TO_RESTORE,
        "should cap at max files"
    );
}

#[test]
fn test_create_post_compact_file_attachments_respects_configured_limit() {
    let dir = tempfile::tempdir().expect("tmpdir");
    let mut snapshot = Vec::new();
    for i in 0..3 {
        let file = dir.path().join(format!("file{i}.rs"));
        std::fs::write(&file, format!("fn f{i}() {{}}")).expect("write");
        snapshot.push((file, make_entry(&format!("fn f{i}() {{}}"), i)));
    }

    let atts = create_post_compact_file_attachments_with_priority_and_limit::<Message>(
        &snapshot,
        &[],
        dir.path(),
        /*plan*/ None,
        &std::collections::HashSet::new(),
        /*max_files_to_restore*/ 1,
    );

    assert_eq!(atts.len(), 1);
    let Some(coco_messages::AttachmentExtras::CompactFileReference(payload)) =
        atts[0].extras.as_ref()
    else {
        panic!("compact file reference should preserve typed display payload");
    };
    assert_eq!(payload.display_path, "file2.rs");
}
