use super::*;
use crate::state::ModalState;
use crate::state::transcript::TranscriptState;
use crate::transcript::derive::test_helpers;
use serde_json::json;
use std::cell::RefCell;

#[test]
fn meta_field_maps_known_tools() {
    assert_eq!(
        meta_field(ToolName::Bash.as_str()),
        Some(("command", "command"))
    );
    assert_eq!(
        meta_field(ToolName::Read.as_str()),
        Some(("file_path", "path"))
    );
    assert_eq!(
        meta_field(ToolName::WebFetch.as_str()),
        Some(("url", "url"))
    );
    assert_eq!(
        meta_field(ToolName::WebSearch.as_str()),
        Some(("query", "query"))
    );
    // NotebookEdit's field is `notebook_path`, not `file_path`.
    assert_eq!(
        meta_field(ToolName::NotebookEdit.as_str()),
        Some(("notebook_path", "path"))
    );
    // MCP / custom tools have no single identifier.
    assert_eq!(meta_field("mcp__slack__send"), None);
}

#[test]
fn cell_meta_extracts_a_bash_command() {
    let cell = test_helpers::tool_use_cell(
        "call-1",
        ToolName::Bash.as_str(),
        json!({ "command": "ls -la" }),
    );
    assert_eq!(cell_meta(&cell), Some(("command", "ls -la".to_string())));
}

#[test]
fn cell_meta_extracts_a_file_path() {
    let cell = test_helpers::tool_use_cell(
        "call-2",
        ToolName::Edit.as_str(),
        json!({ "file_path": "/tmp/x.rs", "old_string": "a", "new_string": "b" }),
    );
    assert_eq!(cell_meta(&cell), Some(("path", "/tmp/x.rs".to_string())));
}

#[test]
fn cell_meta_is_none_for_unmapped_tool() {
    let cell = test_helpers::tool_use_cell("c", "mcp__slack__send", json!({ "text": "hi" }));
    assert_eq!(cell_meta(&cell), None);
}

#[test]
fn cell_meta_extracts_a_notebook_path() {
    let cell = test_helpers::tool_use_cell(
        "call-nb",
        ToolName::NotebookEdit.as_str(),
        json!({ "notebook_path": "/nb.ipynb", "new_source": "x" }),
    );
    assert_eq!(cell_meta(&cell), Some(("path", "/nb.ipynb".to_string())));
}

#[test]
fn cell_text_of_assistant_cell_is_the_prose() {
    let cell = test_helpers::assistant_text_cell("hello world");
    assert_eq!(cell_text(&[], &cell).as_deref(), Some("hello world"));
}

#[test]
fn cell_text_of_thinking_cell_is_the_reasoning() {
    // Regression: a thinking cell must copy its reasoning, not fall through to
    // sibling prose (or nothing for a reasoning-only turn).
    let cell = test_helpers::assistant_thinking_cell("let me reason about this");
    assert_eq!(
        cell_text(&[], &cell).as_deref(),
        Some("let me reason about this")
    );
}

#[test]
fn cell_text_of_tool_use_without_result_is_the_invocation_preview() {
    let cell = test_helpers::tool_use_cell(
        "call-3",
        ToolName::Bash.as_str(),
        json!({ "command": "echo hi" }),
    );
    let text = cell_text(&[], &cell).expect("tool cell yields invocation text");
    assert!(text.contains("echo hi"), "preview was: {text}");
}

#[test]
fn cell_text_of_tool_use_prefers_the_paired_result_output() {
    let use_cell = test_helpers::tool_use_cell(
        "call-4",
        ToolName::Bash.as_str(),
        json!({ "command": "echo hi" }),
    );
    let result_cell = test_helpers::tool_result_cell("call-4", ToolName::Bash.as_str(), "hi\n");
    let cells = vec![use_cell.clone(), result_cell];
    // Selecting the tool cell copies the result output, not the invocation.
    assert_eq!(cell_text(&cells, &use_cell).as_deref(), Some("hi"));
}

#[test]
fn tool_batch_copy_routes_every_tool_result_through_clipboard() {
    let mut state = AppState::default();
    let cells = [
        test_helpers::tool_use_cell(
            "call-a",
            ToolName::Glob.as_str(),
            json!({"pattern": "**/*.rs"}),
        ),
        test_helpers::tool_use_cell(
            "call-b",
            ToolName::Glob.as_str(),
            json!({"pattern": "**/*.md"}),
        ),
        test_helpers::tool_result_cell("call-a", ToolName::Glob.as_str(), "src/lib.rs"),
        test_helpers::tool_result_cell("call-b", ToolName::Glob.as_str(), "README.md"),
    ];
    for cell in cells {
        state.session.transcript.on_message_appended(cell.source);
    }
    let mut transcript = TranscriptState::new();
    transcript.selected_cell_id = Some(TranscriptCellId::tool_batch(0, 2));
    state.ui.modal = Some(ModalState::Transcript(transcript));
    let copied = RefCell::new(String::new());

    assert!(copy_selected_cell_text_with(&mut state, |text| {
        copied.replace(text.to_string());
        Ok(None)
    }));
    assert_eq!(copied.into_inner(), "src/lib.rs\n\nREADME.md");
    assert!(
        state
            .ui
            .toasts
            .back()
            .is_some_and(|toast| toast.message.contains(&"21".to_string())),
        "copy success toast should report the whole batch"
    );
}

#[test]
fn tool_batch_metadata_copy_includes_each_primary_argument() {
    let cells = vec![
        test_helpers::tool_use_cell(
            "call-a",
            ToolName::Bash.as_str(),
            json!({"command": "cargo test"}),
        ),
        test_helpers::tool_use_cell(
            "call-b",
            ToolName::Read.as_str(),
            json!({"file_path": "src/lib.rs"}),
        ),
    ];
    assert_eq!(
        batch_cell_meta(&cells, 0, 2).as_deref(),
        Some("command: cargo test\npath: src/lib.rs")
    );
}
