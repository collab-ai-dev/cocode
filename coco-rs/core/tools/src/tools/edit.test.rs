use crate::tools::edit::EditTool;

// ── R7-T25: edit prompt content check ──
#[tokio::test]
async fn test_edit_prompt_includes_uniqueness_warning() {
    use coco_tool_runtime::PromptOptions;

    // The full guidance lives in the model-facing prompt(); description() is the short label.
    let desc = <EditTool as DynTool>::prompt(&EditTool, &PromptOptions::default()).await;
    assert!(
        desc.contains("must use your `Read` tool"),
        "Edit prompt should warn about read-before-edit requirement"
    );
    assert!(
        desc.contains("FAIL if `old_string` is not unique"),
        "Edit prompt should warn about uniqueness requirement"
    );
    assert!(
        desc.contains("`replace_all`"),
        "Edit prompt should mention replace_all"
    );
}

#[test]
fn test_edit_description_is_short_label() {
    use coco_tool_runtime::DescriptionOptions;
    let fixture = serde_json::json!({"file_path": "/tmp/x", "old_string": "a", "new_string": "b"});
    let desc =
        <EditTool as DynTool>::description(&EditTool, &fixture, &DescriptionOptions::default());
    assert_eq!(desc, "A tool for editing files");
}
use coco_tool_runtime::DynTool;
use coco_tool_runtime::ToolUseContext;
use serde_json::json;

#[tokio::test]
async fn test_edit_single_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.rs");
    std::fs::write(&file, "fn hello() {\n    println!(\"hi\");\n}\n").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <EditTool as DynTool>::execute(
        &EditTool,
        json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "println!(\"hi\")",
            "new_string": "println!(\"hello world\")"
        }),
        &ctx,
    )
    .await
    .unwrap();

    // Output shape: `{filePath, replaceAll, userModified, replacementCount}`.
    assert_eq!(result.data["filePath"], file.to_str().unwrap());
    assert_eq!(result.data["replaceAll"], false);
    assert_eq!(result.data["replacementCount"], 1);
    let content = std::fs::read_to_string(&file).unwrap();
    assert!(content.contains("hello world"));
    assert!(!content.contains("\"hi\""));
}

#[tokio::test]
async fn test_edit_replace_all() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("multi.txt");
    std::fs::write(&file, "foo bar foo baz foo").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <EditTool as DynTool>::execute(
        &EditTool,
        json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "foo",
            "new_string": "qux",
            "replace_all": true
        }),
        &ctx,
    )
    .await
    .unwrap();

    assert_eq!(result.data["replaceAll"], true);
    assert_eq!(result.data["replacementCount"], 3);
    let content = std::fs::read_to_string(&file).unwrap();
    assert_eq!(content, "qux bar qux baz qux");
}

#[tokio::test]
async fn test_edit_not_unique_error() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("dup.txt");
    std::fs::write(&file, "aaa bbb aaa").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <EditTool as DynTool>::execute(
        &EditTool,
        json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "aaa",
            "new_string": "ccc"
        }),
        &ctx,
    )
    .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("2 times"));
}

#[tokio::test]
async fn test_edit_not_found_error() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("miss.txt");
    std::fs::write(&file, "hello world").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <EditTool as DynTool>::execute(
        &EditTool,
        json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "zzz",
            "new_string": "xxx"
        }),
        &ctx,
    )
    .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("not found"));
}

#[tokio::test]
async fn test_edit_same_string_error() {
    let tool: &dyn DynTool = &EditTool;
    let ctx = coco_tool_runtime::ToolUseContext::test_default();
    let result = tool.validate_input(
        &json!({
            "file_path": "/tmp/x.txt",
            "old_string": "same",
            "new_string": "same"
        }),
        &ctx,
    );

    assert!(matches!(
        result,
        coco_tool_runtime::ValidationResult::Invalid { .. }
    ));
}

#[tokio::test]
async fn test_edit_file_not_found() {
    let ctx = ToolUseContext::test_default();
    let result = <EditTool as DynTool>::execute(
        &EditTool,
        json!({
            "file_path": "/nonexistent/file.txt",
            "old_string": "a",
            "new_string": "b"
        }),
        &ctx,
    )
    .await;

    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// B2.4: quote normalization + content fallback race
// ---------------------------------------------------------------------------

/// When the file has curly quotes ("smart" quotes) but the model emitted
/// straight quotes, the match must succeed via quote normalization.
#[tokio::test]
async fn test_edit_matches_curly_quotes() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("curly.md");
    // File uses left/right curly double quotes: \u{201C} hello \u{201D}
    let curly_content = "say \u{201C}hello\u{201D} to the world";
    std::fs::write(&file, curly_content).unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <EditTool as DynTool>::execute(
        &EditTool,
        json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "\"hello\"",  // Model uses straight quotes
            "new_string": "\"hi\""
        }),
        &ctx,
    )
    .await
    .unwrap();

    // Structured result: presence of `filePath` is sufficient — the
    // model-visible message is exercised in render_for_model tests.
    assert_eq!(result.data["filePath"], file.to_str().unwrap());
    let content = std::fs::read_to_string(&file).unwrap();
    // preserve_quote_style should have re-applied curly quotes to new_string.
    assert!(
        content.contains('\u{201C}') && content.contains('\u{201D}'),
        "curly quotes should be preserved: {content}"
    );
    assert!(content.contains("hi"));
}

/// Single curly quote (apostrophe) variant.
#[tokio::test]
async fn test_edit_matches_curly_single_quotes() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("apostrophe.txt");
    // Using right-single curly (typographic apostrophe).
    let content = "it\u{2019}s a test";
    std::fs::write(&file, content).unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <EditTool as DynTool>::execute(
        &EditTool,
        json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "it's",
            "new_string": "that's"
        }),
        &ctx,
    )
    .await
    .unwrap();

    // Structured result: presence of `filePath` is sufficient — the
    // model-visible message is exercised in render_for_model tests.
    assert_eq!(result.data["filePath"], file.to_str().unwrap());
    let content = std::fs::read_to_string(&file).unwrap();
    assert!(
        content.contains("that\u{2019}s"),
        "curly apostrophe preserved: {content}"
    );
}

// ---------------------------------------------------------------------------
// B2.4: content-fallback race detection
// ---------------------------------------------------------------------------

use coco_context::FileReadEntry;
use coco_context::FileReadState;
use std::sync::Arc;
use tokio::sync::RwLock;

// ---------------------------------------------------------------------------
// T2: normalize_file_edit_input integration (trailing whitespace + desanitize)
// ---------------------------------------------------------------------------

/// Model emits `new_string` with trailing spaces on each line. The edit
/// tool strips them before applying (except for .md files). Regression
/// guard: the strip must happen automatically so the model doesn't have
/// to be careful.
#[tokio::test]
async fn test_edit_strips_trailing_whitespace_from_new_string() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("code.rs");
    std::fs::write(&file, "fn hello() {\n    println!(\"hi\");\n}\n").unwrap();

    let ctx = ToolUseContext::test_default();
    // Note the trailing spaces on `println!("hello");   ` and on the closing brace.
    let result = <EditTool as DynTool>::execute(
        &EditTool,
        json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "    println!(\"hi\");\n}",
            "new_string": "    println!(\"hello\");   \n}  "
        }),
        &ctx,
    )
    .await
    .unwrap();
    assert!(
        result.data["filePath"].is_string(),
        "expected filePath in data: {:?}",
        result.data
    );

    // Trailing spaces should NOT end up on disk.
    let content = std::fs::read_to_string(&file).unwrap();
    assert!(content.contains("println!(\"hello\")"));
    assert!(
        !content.contains("hello\");   "),
        "trailing whitespace after `hello\");` must be stripped; got: {content:?}"
    );
    assert!(
        !content.contains("}  "),
        "trailing whitespace after `}}` must be stripped; got: {content:?}"
    );
}

/// Markdown files MUST preserve trailing whitespace — two trailing
/// spaces in markdown is a hard line break. The `.md` / `.mdx`
/// extensions skip the whitespace strip.
#[tokio::test]
async fn test_edit_preserves_trailing_whitespace_in_markdown() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes.md");
    std::fs::write(&file, "line one\nline two\n").unwrap();

    let ctx = ToolUseContext::test_default();
    // Replace with content that has trailing 2-space hard line break.
    let result = <EditTool as DynTool>::execute(
        &EditTool,
        json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "line one",
            "new_string": "line one  "
        }),
        &ctx,
    )
    .await
    .unwrap();
    assert!(
        result.data["filePath"].is_string(),
        "expected filePath in data: {:?}",
        result.data
    );

    let content = std::fs::read_to_string(&file).unwrap();
    assert!(
        content.contains("line one  "),
        "markdown files must preserve trailing hard-break spaces; got: {content:?}"
    );
}

/// Case-insensitive extension check: `.MD` and `.mdx` also preserve.
#[tokio::test]
async fn test_edit_markdown_extension_case_insensitive() {
    let dir = tempfile::tempdir().unwrap();
    for ext in &["MD", "mdx", "MdX"] {
        let file = dir.path().join(format!("doc.{ext}"));
        std::fs::write(&file, "hello\n").unwrap();

        let ctx = ToolUseContext::test_default();
        let result = <EditTool as DynTool>::execute(
            &EditTool,
            json!({
                "file_path": file.to_str().unwrap(),
                "old_string": "hello",
                "new_string": "world  "
            }),
            &ctx,
        )
        .await
        .unwrap();
        assert!(
            result.data["filePath"].is_string(),
            "expected filePath in data: {:?}",
            result.data
        );

        let content = std::fs::read_to_string(&file).unwrap();
        assert!(
            content.contains("world  "),
            "extension .{ext} must match case-insensitive markdown rule; got: {content:?}"
        );
    }
}

/// Desanitization: model emits `<n>` / `</n>` instead of `<name>` /
/// `</name>`. Both open and close tag forms are in the desanitization
/// map, so `normalizeFileEditInput` rewrites old_string to match the
/// real file content.
#[tokio::test]
async fn test_edit_desanitizes_sanitized_tags_in_old_string() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("log.txt");
    // File contains the REAL un-sanitized tag.
    std::fs::write(&file, "before\n<name>OK</name>\nafter\n").unwrap();

    let ctx = ToolUseContext::test_default();
    // Model emits the SANITIZED form `<n>`/`</n>` — the raw string
    // won't match the file, but desanitization rewrites it to
    // `<name>`/`</name>` before matching, which DOES match. Then the
    // same rewrite is applied to new_string so the final edit
    // replaces the real tag with a real tag.
    let result = <EditTool as DynTool>::execute(
        &EditTool,
        json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "<n>OK</n>",
            "new_string": "<n>UPDATED</n>"
        }),
        &ctx,
    )
    .await
    .unwrap();
    assert!(
        result.data["filePath"].is_string(),
        "expected filePath in data: {:?}",
        result.data
    );

    let content = std::fs::read_to_string(&file).unwrap();
    // The replacement should use the REAL tags, not the sanitized form.
    assert!(
        content.contains("<name>UPDATED</name>"),
        "desanitized new_string should write real tags; got: {content:?}"
    );
    assert!(
        !content.contains("<n>"),
        "sanitized form should NOT leak to disk; got: {content:?}"
    );
}

/// When file_read_state is primed with STALE content but the mtime
/// matches current disk (e.g. two edits within 1-second mtime precision
/// window), the content-fallback check must catch the drift and reject.
#[tokio::test]
async fn test_edit_detects_content_drift_in_race() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("race.txt");
    std::fs::write(&file, "current on disk").unwrap();
    let abs = std::fs::canonicalize(&file).unwrap();
    let mtime = coco_context::file_mtime_ms(&abs).await.unwrap();

    let mut ctx = ToolUseContext::test_default();
    ctx.file_read_state = Some(Arc::new(RwLock::new(FileReadState::new())));
    {
        let mut frs = ctx.file_read_state.as_ref().unwrap().write().await;
        frs.set(
            abs,
            FileReadEntry::full_real(
                "stale cached content".into(), // != current
                mtime,                         // but same mtime
            ),
        );
    }

    let result = <EditTool as DynTool>::execute(
        &EditTool,
        json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "current",
            "new_string": "changed"
        }),
        &ctx,
    )
    .await;

    assert!(result.is_err(), "content drift must be detected");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("modified") || err.contains("content"),
        "error should mention drift: {err}"
    );
    // Content on disk must be unchanged.
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "current on disk");
}

#[tokio::test]
async fn test_edit_allows_newer_mtime_when_full_content_is_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("same-content.txt");
    std::fs::write(&file, "hello world\n").unwrap();
    let abs = std::fs::canonicalize(&file).unwrap();

    let mut ctx = ToolUseContext::test_default();
    ctx.file_read_state = Some(Arc::new(RwLock::new(FileReadState::new())));
    {
        let mut frs = ctx.file_read_state.as_ref().unwrap().write().await;
        frs.set(abs, FileReadEntry::full_real("hello world\n".into(), 0));
    }

    let result = <EditTool as DynTool>::execute(
        &EditTool,
        json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "hello",
            "new_string": "goodbye"
        }),
        &ctx,
    )
    .await;

    assert!(
        result.is_ok(),
        "newer mtime with identical full content should be allowed: {result:?}"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "goodbye world\n");
}

#[tokio::test]
async fn test_edit_rejects_newer_mtime_when_full_content_changed() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("changed-content.txt");
    std::fs::write(&file, "current on disk\n").unwrap();
    let abs = std::fs::canonicalize(&file).unwrap();

    let mut ctx = ToolUseContext::test_default();
    ctx.file_read_state = Some(Arc::new(RwLock::new(FileReadState::new())));
    {
        let mut frs = ctx.file_read_state.as_ref().unwrap().write().await;
        frs.set(abs, FileReadEntry::full_real("cached version\n".into(), 0));
    }

    let result = <EditTool as DynTool>::execute(
        &EditTool,
        json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "current",
            "new_string": "updated"
        }),
        &ctx,
    )
    .await;

    assert!(result.is_err(), "changed content must be rejected");
    assert!(
        result.unwrap_err().to_string().contains("content changed"),
        "error should mention content drift",
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "current on disk\n");
}

/// Read-before-edit guard (errorCode 6): when the file exists on disk but
/// has NO FileReadState entry, the edit must be rejected — editing an
/// unseen file is the data-loss class this guards.
#[tokio::test]
async fn test_edit_rejects_unread_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("unread.txt");
    std::fs::write(&file, "hello world").unwrap();

    let mut ctx = ToolUseContext::test_default();
    // Empty FileReadState — the file was never read this session.
    ctx.file_read_state = Some(Arc::new(RwLock::new(FileReadState::new())));

    let result = <EditTool as DynTool>::execute(
        &EditTool,
        json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "hello",
            "new_string": "goodbye"
        }),
        &ctx,
    )
    .await;

    assert!(result.is_err(), "editing an unread file must be rejected");
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("has not been read"),
        "error should tell the model to read first",
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello world");
}

/// Line-range reads from the Read tool are valid edit evidence as long as the
/// file has not advanced on disk.
#[tokio::test]
async fn test_edit_allows_line_range_read() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("partial.txt");
    std::fs::write(&file, "line one\nline two\n").unwrap();
    let abs = std::fs::canonicalize(&file).unwrap();
    let mtime = coco_context::file_mtime_ms(&abs).await.unwrap();

    let mut ctx = ToolUseContext::test_default();
    ctx.file_read_state = Some(Arc::new(RwLock::new(FileReadState::new())));
    {
        let mut frs = ctx.file_read_state.as_ref().unwrap().write().await;
        frs.set(
            abs,
            FileReadEntry::line_real("line one\n".into(), mtime, None, 1),
        );
    }

    let result = <EditTool as DynTool>::execute(
        &EditTool,
        json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "line one",
            "new_string": "LINE ONE"
        }),
        &ctx,
    )
    .await;

    assert!(
        result.is_ok(),
        "editing after a line-range read should be allowed: {result:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "LINE ONE\nline two\n"
    );
}

#[tokio::test]
async fn test_edit_rejects_injected_partial_view() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("partial.txt");
    std::fs::write(&file, "line one\nline two\n").unwrap();
    let abs = std::fs::canonicalize(&file).unwrap();
    let mtime = coco_context::file_mtime_ms(&abs).await.unwrap();

    let mut ctx = ToolUseContext::test_default();
    ctx.file_read_state = Some(Arc::new(RwLock::new(FileReadState::new())));
    {
        let mut frs = ctx.file_read_state.as_ref().unwrap().write().await;
        frs.set(
            abs,
            FileReadEntry::injected_partial(
                "line one\n".into(),
                mtime,
                coco_context::FileReadRange::Lines {
                    offset: None,
                    limit: 1,
                },
            ),
        );
    }

    let result = <EditTool as DynTool>::execute(
        &EditTool,
        json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "line one",
            "new_string": "LINE ONE"
        }),
        &ctx,
    )
    .await;

    assert!(result.is_err(), "injected partial views must be rejected");
    assert!(
        result.unwrap_err().to_string().contains("partial injected"),
        "error should mention injected partial context",
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "line one\nline two\n"
    );
}

#[tokio::test]
async fn test_edit_rejects_stale_line_range_read() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("partial.txt");
    std::fs::write(&file, "line one\nline two\n").unwrap();
    let abs = std::fs::canonicalize(&file).unwrap();

    let mut ctx = ToolUseContext::test_default();
    ctx.file_read_state = Some(Arc::new(RwLock::new(FileReadState::new())));
    {
        let mut frs = ctx.file_read_state.as_ref().unwrap().write().await;
        frs.set(
            abs,
            FileReadEntry::line_real("line one\n".into(), 0, None, 1),
        );
    }

    let result = <EditTool as DynTool>::execute(
        &EditTool,
        json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "line one",
            "new_string": "LINE ONE"
        }),
        &ctx,
    )
    .await;

    assert!(result.is_err(), "stale line-range reads must be rejected");
    assert!(
        result.unwrap_err().to_string().contains("mtime changed"),
        "error should mention mtime drift",
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "line one\nline two\n"
    );
}

// ---------------------------------------------------------------------------
// render_for_model — Edit branches
// ---------------------------------------------------------------------------

#[test]
fn edit_render_single_replacement_branch() {
    use coco_tool_runtime::ToolResultContentPart;
    let data = json!({"filePath": "/abs/file.rs", "replaceAll": false, "userModified": false});
    let parts = <EditTool as DynTool>::render_for_model(&EditTool, &data);
    let ToolResultContentPart::Text { text, .. } = &parts[0] else {
        panic!("expected Text part");
    };
    assert_eq!(text, "The file /abs/file.rs has been updated successfully.");
}

#[test]
fn edit_render_replace_all_branch() {
    use coco_tool_runtime::ToolResultContentPart;
    let data = json!({"filePath": "/abs/multi.rs", "replaceAll": true, "userModified": false});
    let parts = <EditTool as DynTool>::render_for_model(&EditTool, &data);
    let ToolResultContentPart::Text { text, .. } = &parts[0] else {
        panic!("expected Text part");
    };
    assert_eq!(
        text,
        "The file /abs/multi.rs has been updated. All occurrences were successfully replaced."
    );
}

// ---------------------------------------------------------------------------
// #21 — empty old_string creates a new file
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_edit_creates_new_file_with_empty_old_string() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("sub/new.txt"); // parent dir does not exist yet
    let ctx = ToolUseContext::test_default();
    let result = <EditTool as DynTool>::execute(
        &EditTool,
        json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "",
            "new_string": "brand new content\n"
        }),
        &ctx,
    )
    .await
    .unwrap();
    assert_eq!(result.data["replacementCount"], 1);
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "brand new content\n"
    );
}

// ---------------------------------------------------------------------------
// #22 — deletion strips the trailing newline
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_edit_deletion_strips_trailing_newline() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("del.txt");
    std::fs::write(&file, "keep\nremove me\ntail\n").unwrap();
    let ctx = ToolUseContext::test_default();
    <EditTool as DynTool>::execute(
        &EditTool,
        json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "remove me", // no trailing newline
            "new_string": ""
        }),
        &ctx,
    )
    .await
    .unwrap();
    // The "remove me\n" line is removed whole — no orphan blank line.
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "keep\ntail\n");
}

// ---------------------------------------------------------------------------
// #26 — Edit rejects .ipynb files (route to NotebookEdit)
// ---------------------------------------------------------------------------

#[test]
fn test_edit_rejects_ipynb() {
    let ctx = ToolUseContext::test_default();
    let res = <EditTool as DynTool>::validate_input(
        &EditTool,
        &json!({"file_path": "/work/nb.ipynb", "old_string": "a", "new_string": "b"}),
        &ctx,
    );
    match res {
        coco_tool_runtime::ValidationResult::Invalid {
            error_code,
            message,
        } => {
            assert_eq!(error_code.as_deref(), Some("5"));
            assert!(message.contains("NotebookEdit"), "got: {message}");
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
}
