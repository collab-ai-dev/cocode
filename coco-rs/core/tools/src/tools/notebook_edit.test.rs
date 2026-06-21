//! Tests for NotebookEditTool (B3.1) and the `generate_cell_id` helper.

use super::NotebookEditTool;
use super::generate_cell_id;
use coco_tool_runtime::DynTool;
use coco_tool_runtime::ToolUseContext;
use serde_json::json;

// ---------------------------------------------------------------------------
// Auto-mode classifier projection (`NotebookEditTool.toAutoClassifierInput`)
// ---------------------------------------------------------------------------

#[test]
fn test_notebook_edit_classifier_input_includes_edit_mode() {
    // `${notebook_path} ${mode}: ${new_source}` — `edit_mode` is
    // security-relevant (insert / replace / delete) so it rides along.
    assert_eq!(
        <NotebookEditTool as DynTool>::to_auto_classifier_input(
            &NotebookEditTool,
            &json!({
                "notebook_path": "/work/nb.ipynb",
                "cell_id": "c1",
                "new_source": "print('x')",
                "edit_mode": "delete"
            }),
        ),
        Some("/work/nb.ipynb delete: print('x')".to_string())
    );
}

#[test]
fn test_notebook_edit_classifier_input_defaults_to_replace() {
    // Omitted `edit_mode` defaults to `replace` (serde default).
    assert_eq!(
        <NotebookEditTool as DynTool>::to_auto_classifier_input(
            &NotebookEditTool,
            &json!({
                "notebook_path": "/work/nb.ipynb",
                "cell_id": "c1",
                "new_source": "y = 1"
            }),
        ),
        Some("/work/nb.ipynb replace: y = 1".to_string())
    );
}

// ---------------------------------------------------------------------------
// generate_cell_id — 13-char base-36 format
// ---------------------------------------------------------------------------

#[test]
fn test_generate_cell_id_length() {
    let id = generate_cell_id();
    assert_eq!(id.len(), 13, "uses 13-char IDs: got {id}");
}

#[test]
fn test_generate_cell_id_charset() {
    // Base-36 means lowercase alphanumeric only.
    for _ in 0..100 {
        let id = generate_cell_id();
        for c in id.chars() {
            assert!(
                c.is_ascii_lowercase() || c.is_ascii_digit(),
                "cell ID contains invalid char: '{c}' in '{id}'"
            );
        }
    }
}

#[test]
fn test_generate_cell_id_uniqueness() {
    // 100 IDs should all be unique with overwhelming probability.
    let ids: std::collections::HashSet<_> = (0..100).map(|_| generate_cell_id()).collect();
    assert_eq!(ids.len(), 100, "all 100 generated IDs should be unique");
}

// ---------------------------------------------------------------------------
// Helper: build a minimal valid notebook
// ---------------------------------------------------------------------------

fn minimal_notebook(nbformat_minor: i64) -> serde_json::Value {
    json!({
        "nbformat": 4,
        "nbformat_minor": nbformat_minor,
        "metadata": {},
        "cells": [
            {
                "cell_type": "code",
                "source": ["print('hello')\n"],
                "metadata": {},
                "execution_count": null,
                "outputs": []
            },
            {
                "cell_type": "markdown",
                "source": ["# Intro\n"],
                "metadata": {}
            }
        ]
    })
}

fn write_notebook(content: &serde_json::Value) -> tempfile::NamedTempFile {
    let file = tempfile::Builder::new()
        .suffix(".ipynb")
        .tempfile()
        .unwrap();
    std::fs::write(file.path(), serde_json::to_string_pretty(content).unwrap()).unwrap();
    file
}

// ---------------------------------------------------------------------------
// replace mode
// ---------------------------------------------------------------------------

/// Replace by numeric cell index. allows `cell_id` to be an integer
/// string as well as a cell UUID — our resolver handles both.
#[tokio::test]
async fn test_notebook_replace_by_index() {
    let notebook = minimal_notebook(5);
    let file = write_notebook(&notebook);

    let ctx = ToolUseContext::test_default();
    let result = <NotebookEditTool as DynTool>::execute(
        &NotebookEditTool,
        json!({
            "notebook_path": file.path().to_str().unwrap(),
            "cell_id": "0",
            "edit_mode": "replace",
            "new_source": "print('goodbye')"
        }),
        &ctx,
    )
    .await
    .unwrap();

    assert_eq!(result.data["edit_mode"], "replace");
    assert_eq!(result.data["cell_index"], 0);

    let updated: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(file.path()).unwrap()).unwrap();
    let source = updated["cells"][0]["source"].as_array().unwrap();
    assert!(source[0].as_str().unwrap().contains("goodbye"));
    // Replace must reset execution state.
    assert!(updated["cells"][0]["execution_count"].is_null());
    assert!(
        updated["cells"][0]["outputs"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

/// Replace by `cell-N` prefix (coco-rs back-compat alias).
#[tokio::test]
async fn test_notebook_replace_by_cell_prefix() {
    let notebook = minimal_notebook(5);
    let file = write_notebook(&notebook);

    let ctx = ToolUseContext::test_default();
    let result = <NotebookEditTool as DynTool>::execute(
        &NotebookEditTool,
        json!({
            "notebook_path": file.path().to_str().unwrap(),
            "cell_id": "cell-1",
            "edit_mode": "replace",
            "new_source": "# Updated"
        }),
        &ctx,
    )
    .await
    .unwrap();

    assert_eq!(result.data["cell_index"], 1);
    let updated: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(file.path()).unwrap()).unwrap();
    let source = updated["cells"][1]["source"].as_array().unwrap();
    assert!(source[0].as_str().unwrap().contains("Updated"));
}

/// Replace by actual cell UUID (the primary path).
#[tokio::test]
async fn test_notebook_replace_by_uuid() {
    let mut notebook = minimal_notebook(5);
    notebook["cells"][0]["id"] = json!("abc123xyz");
    let file = write_notebook(&notebook);

    let ctx = ToolUseContext::test_default();
    let result = <NotebookEditTool as DynTool>::execute(
        &NotebookEditTool,
        json!({
            "notebook_path": file.path().to_str().unwrap(),
            "cell_id": "abc123xyz",
            "edit_mode": "replace",
            "new_source": "# by uuid"
        }),
        &ctx,
    )
    .await
    .unwrap();

    assert_eq!(result.data["cell_index"], 0);
}

// ---------------------------------------------------------------------------
// insert mode
// ---------------------------------------------------------------------------

/// Inserting into a nbformat 4.5+ notebook must auto-generate a cell ID
/// in 13-char base-36 format.
#[tokio::test]
async fn test_notebook_insert_auto_generates_id_on_45() {
    let notebook = minimal_notebook(5);
    let file = write_notebook(&notebook);

    let ctx = ToolUseContext::test_default();
    <NotebookEditTool as DynTool>::execute(
        &NotebookEditTool,
        json!({
            "notebook_path": file.path().to_str().unwrap(),
            "cell_id": "0",
            "edit_mode": "insert",
            "cell_type": "code",
            "new_source": "x = 42"
        }),
        &ctx,
    )
    .await
    .unwrap();

    let updated: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(file.path()).unwrap()).unwrap();
    // Inserted AFTER cell 0 (cell_id "0"), so the new cell lands at index 1.
    let id = updated["cells"][1]["id"].as_str().unwrap();
    assert_eq!(id.len(), 13);
    // Must be lowercase alphanumeric.
    assert!(
        id.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    );
}

/// Inserting into a pre-4.5 notebook must NOT add a cell ID.
#[tokio::test]
async fn test_notebook_insert_no_id_on_pre_45() {
    let notebook = minimal_notebook(4); // nbformat_minor = 4
    let file = write_notebook(&notebook);

    let ctx = ToolUseContext::test_default();
    <NotebookEditTool as DynTool>::execute(
        &NotebookEditTool,
        json!({
            "notebook_path": file.path().to_str().unwrap(),
            "cell_id": "0",
            "edit_mode": "insert",
            "cell_type": "markdown",
            "new_source": "# Title"
        }),
        &ctx,
    )
    .await
    .unwrap();

    let updated: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(file.path()).unwrap()).unwrap();
    // Inserted AFTER cell 0 → new cell at index 1.
    assert!(
        updated["cells"][1].get("id").is_none() || updated["cells"][1]["id"].is_null(),
        "pre-4.5 notebooks must not add cell IDs"
    );
}

/// Inserting a markdown cell must NOT include execution_count/outputs
/// (those are code-cell only).
#[tokio::test]
async fn test_notebook_insert_markdown_has_no_execution_state() {
    let notebook = minimal_notebook(5);
    let file = write_notebook(&notebook);

    let ctx = ToolUseContext::test_default();
    <NotebookEditTool as DynTool>::execute(
        &NotebookEditTool,
        json!({
            "notebook_path": file.path().to_str().unwrap(),
            "cell_id": "0",
            "edit_mode": "insert",
            "cell_type": "markdown",
            "new_source": "# heading"
        }),
        &ctx,
    )
    .await
    .unwrap();

    let updated: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(file.path()).unwrap()).unwrap();
    // Inserted AFTER cell 0 → new cell at index 1.
    let cell = &updated["cells"][1];
    assert_eq!(cell["cell_type"], "markdown");
    assert!(cell.get("execution_count").is_none());
    assert!(cell.get("outputs").is_none());
}

/// Regression for tools-file#18: insert places the new cell AFTER the
/// referenced cell, not before it. With `cell_id: "0"` the new cell must
/// land at index 1; an empty `cell_id` inserts at the beginning (index 0).
#[tokio::test]
async fn test_notebook_insert_lands_after_referenced_cell() {
    let file = write_notebook(&minimal_notebook(5));
    let ctx = ToolUseContext::test_default();
    <NotebookEditTool as DynTool>::execute(
        &NotebookEditTool,
        json!({
            "notebook_path": file.path().to_str().unwrap(),
            "cell_id": "0",
            "edit_mode": "insert",
            "cell_type": "code",
            "new_source": "INSERTED"
        }),
        &ctx,
    )
    .await
    .unwrap();

    let updated: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(file.path()).unwrap()).unwrap();
    // Original cell 0 stays first; the new cell is right after it.
    assert_eq!(updated["cells"][0]["source"][0], "print('hello')\n");
    assert_eq!(updated["cells"][1]["source"][0], "INSERTED");
}

/// Empty `cell_id` inserts at the beginning (index 0), unaffected by the
/// insert-after `+1`.
#[tokio::test]
async fn test_notebook_insert_empty_cell_id_prepends() {
    let file = write_notebook(&minimal_notebook(5));
    let ctx = ToolUseContext::test_default();
    <NotebookEditTool as DynTool>::execute(
        &NotebookEditTool,
        json!({
            "notebook_path": file.path().to_str().unwrap(),
            "cell_id": "",
            "edit_mode": "insert",
            "cell_type": "code",
            "new_source": "FIRST"
        }),
        &ctx,
    )
    .await
    .unwrap();

    let updated: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(file.path()).unwrap()).unwrap();
    assert_eq!(updated["cells"][0]["source"][0], "FIRST");
}

// ---------------------------------------------------------------------------
// delete mode
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_notebook_delete_by_index() {
    let notebook = minimal_notebook(5);
    let file = write_notebook(&notebook);

    let ctx = ToolUseContext::test_default();
    <NotebookEditTool as DynTool>::execute(
        &NotebookEditTool,
        json!({
            "notebook_path": file.path().to_str().unwrap(),
            "cell_id": "1",
            "new_source": "",
            "edit_mode": "delete"
        }),
        &ctx,
    )
    .await
    .unwrap();

    let updated: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(file.path()).unwrap()).unwrap();
    let cells = updated["cells"].as_array().unwrap();
    assert_eq!(cells.len(), 1);
    // Only the first (code) cell should remain.
    assert_eq!(cells[0]["cell_type"], "code");
}

// ---------------------------------------------------------------------------
// Error paths
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_notebook_missing_path() {
    let ctx = ToolUseContext::test_default();
    let result =
        <NotebookEditTool as DynTool>::execute(&NotebookEditTool, json!({"cell_id": "0"}), &ctx)
            .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_notebook_invalid_edit_mode() {
    let notebook = minimal_notebook(5);
    let file = write_notebook(&notebook);

    let ctx = ToolUseContext::test_default();
    let result = <NotebookEditTool as DynTool>::execute(
        &NotebookEditTool,
        json!({
            "notebook_path": file.path().to_str().unwrap(),
            "cell_id": "0",
            "edit_mode": "reorder"
        }),
        &ctx,
    )
    .await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("edit_mode") || err.contains("reorder"));
}

#[tokio::test]
async fn test_notebook_cell_index_out_of_range() {
    let notebook = minimal_notebook(5);
    let file = write_notebook(&notebook);

    let ctx = ToolUseContext::test_default();
    let result = <NotebookEditTool as DynTool>::execute(
        &NotebookEditTool,
        json!({
            "notebook_path": file.path().to_str().unwrap(),
            "cell_id": "99",
            "edit_mode": "replace",
            "new_source": "x"
        }),
        &ctx,
    )
    .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_notebook_malformed_file() {
    let file = tempfile::Builder::new()
        .suffix(".ipynb")
        .tempfile()
        .unwrap();
    std::fs::write(file.path(), "{ not valid json").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <NotebookEditTool as DynTool>::execute(
        &NotebookEditTool,
        json!({
            "notebook_path": file.path().to_str().unwrap(),
            "cell_id": "0",
            "edit_mode": "replace",
            "new_source": "x"
        }),
        &ctx,
    )
    .await;
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// B3.3 / R4-T4: read-before-edit enforcement. Without this the model can
// silently overwrite notebooks it never saw. We only enforce it when
// `file_read_state` is wired — the default test context leaves it `None`,
// which is why the existing tests above still pass.
// ---------------------------------------------------------------------------

use coco_context::FileReadEntry;
use coco_context::FileReadState;
use std::sync::Arc;
use tokio::sync::RwLock;

/// With a populated `file_read_state` cache but no entry for this notebook,
/// the edit must be rejected before any disk write.
#[tokio::test]
async fn test_notebook_rejects_edit_without_prior_read() {
    let notebook = minimal_notebook(5);
    let file = write_notebook(&notebook);

    let mut ctx = ToolUseContext::test_default();
    ctx.file_read_state = Some(Arc::new(RwLock::new(FileReadState::new())));

    let result = <NotebookEditTool as DynTool>::execute(
        &NotebookEditTool,
        json!({
            "notebook_path": file.path().to_str().unwrap(),
            "cell_id": "0",
            "edit_mode": "replace",
            "new_source": "print('blocked')"
        }),
        &ctx,
    )
    .await;

    assert!(result.is_err(), "edit without prior read must fail");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("has not been read yet"),
        "error should mention missing read: {err}"
    );

    // And the file on disk must be unchanged.
    let after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(file.path()).unwrap()).unwrap();
    let source = after["cells"][0]["source"].as_array().unwrap();
    assert!(
        source[0].as_str().unwrap().contains("hello"),
        "file must not be modified when read-before-edit fails"
    );
}

/// With a primed `file_read_state` entry matching the current mtime, the
/// edit must succeed (mtime drift check passes).
#[tokio::test]
async fn test_notebook_allows_edit_after_read() {
    let notebook = minimal_notebook(5);
    let file = write_notebook(&notebook);
    let abs = std::fs::canonicalize(file.path()).unwrap();
    let mtime = coco_context::file_mtime_ms(&abs).await.unwrap();

    let mut ctx = ToolUseContext::test_default();
    ctx.file_read_state = Some(Arc::new(RwLock::new(FileReadState::new())));
    {
        let mut frs = ctx.file_read_state.as_ref().unwrap().write().await;
        frs.set(
            abs,
            FileReadEntry::full_real(std::fs::read_to_string(file.path()).unwrap(), mtime),
        );
    }

    let result = <NotebookEditTool as DynTool>::execute(
        &NotebookEditTool,
        json!({
            "notebook_path": file.path().to_str().unwrap(),
            "cell_id": "0",
            "edit_mode": "replace",
            "new_source": "print('allowed')"
        }),
        &ctx,
    )
    .await
    .expect("edit with prior read must succeed");
    assert_eq!(result.data["cell_index"], 0);
}

// ---------------------------------------------------------------------------
// R5-T15: return shape — `new_cell_id` on insert, `cell_id` on replace/delete
// ---------------------------------------------------------------------------

/// Insert mode on a nbformat 4.5 notebook emits a `new_cell_id` field
/// containing the freshly-generated 13-char base-36 ID.
#[tokio::test]
async fn test_notebook_insert_returns_new_cell_id() {
    let notebook = minimal_notebook(5); // 4.5 → IDs generated
    let file = write_notebook(&notebook);

    let ctx = ToolUseContext::test_default();
    let result = <NotebookEditTool as DynTool>::execute(
        &NotebookEditTool,
        json!({
            "notebook_path": file.path().to_str().unwrap(),
            "cell_id": "0",
            "edit_mode": "insert",
            "cell_type": "code",
            "new_source": "x = 1"
        }),
        &ctx,
    )
    .await
    .unwrap();

    let new_id = result.data["new_cell_id"].as_str().unwrap();
    assert_eq!(new_id.len(), 13, "cell IDs are 13 chars: got {new_id}");
    // ID must consist of base-36 alphanumerics only.
    assert!(
        new_id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    );
}

/// Insert on a pre-4.5 notebook emits `new_cell_id: null` (ID generation
/// omitted for these notebooks).
#[tokio::test]
async fn test_notebook_insert_pre_45_emits_null_new_cell_id() {
    let notebook = minimal_notebook(0); // 4.0 → no IDs
    let file = write_notebook(&notebook);

    let ctx = ToolUseContext::test_default();
    let result = <NotebookEditTool as DynTool>::execute(
        &NotebookEditTool,
        json!({
            "notebook_path": file.path().to_str().unwrap(),
            "cell_id": "0",
            "edit_mode": "insert",
            "cell_type": "markdown",
            "new_source": "# header"
        }),
        &ctx,
    )
    .await
    .unwrap();

    assert!(result.data["new_cell_id"].is_null());
}

/// Replace mode echoes back the resolved `cell_id` string when the
/// target cell has one — the model can reference cells by the ID it
/// just wrote.
#[tokio::test]
async fn test_notebook_replace_returns_cell_id() {
    let mut notebook = minimal_notebook(5);
    notebook["cells"][0]["id"] = json!("abc123xyz");
    let file = write_notebook(&notebook);

    let ctx = ToolUseContext::test_default();
    let result = <NotebookEditTool as DynTool>::execute(
        &NotebookEditTool,
        json!({
            "notebook_path": file.path().to_str().unwrap(),
            "cell_id": "abc123xyz",
            "edit_mode": "replace",
            "new_source": "# updated"
        }),
        &ctx,
    )
    .await
    .unwrap();

    assert_eq!(result.data["cell_id"], "abc123xyz");
    // And the index is still returned for debuggability.
    assert_eq!(result.data["cell_index"], 0);
}

/// Delete mode echoes back the deleted cell's id.
#[tokio::test]
async fn test_notebook_delete_returns_cell_id() {
    let mut notebook = minimal_notebook(5);
    notebook["cells"][1]["id"] = json!("target-cell-id");
    let file = write_notebook(&notebook);

    let ctx = ToolUseContext::test_default();
    let result = <NotebookEditTool as DynTool>::execute(
        &NotebookEditTool,
        json!({
            "notebook_path": file.path().to_str().unwrap(),
            "cell_id": "target-cell-id",
            "new_source": "",
            "edit_mode": "delete"
        }),
        &ctx,
    )
    .await
    .unwrap();

    assert_eq!(result.data["cell_id"], "target-cell-id");
}

/// When `file_read_state` contains a stale entry whose `mtime_ms` does not
/// match disk, the edit must be rejected to protect against external
/// mutations.
#[tokio::test]
async fn test_notebook_rejects_edit_on_mtime_drift() {
    let notebook = minimal_notebook(5);
    let file = write_notebook(&notebook);
    let abs = std::fs::canonicalize(file.path()).unwrap();
    let real_mtime = coco_context::file_mtime_ms(&abs).await.unwrap();

    let mut ctx = ToolUseContext::test_default();
    ctx.file_read_state = Some(Arc::new(RwLock::new(FileReadState::new())));
    {
        let mut frs = ctx.file_read_state.as_ref().unwrap().write().await;
        frs.set(
            abs,
            FileReadEntry::full_real(
                "stale view".into(),
                // Deliberately wrong mtime to simulate external modification.
                real_mtime.saturating_sub(10_000),
            ),
        );
    }

    let result = <NotebookEditTool as DynTool>::execute(
        &NotebookEditTool,
        json!({
            "notebook_path": file.path().to_str().unwrap(),
            "cell_id": "0",
            "edit_mode": "replace",
            "new_source": "print('should fail')"
        }),
        &ctx,
    )
    .await;

    assert!(result.is_err(), "stale mtime must block edit");
    assert!(
        result.unwrap_err().to_string().contains("modified"),
        "error should mention modification"
    );
}

// ── render_for_model — cell-edit confirmations ───────────────────────

/// Replace on a 4.5+ notebook must surface the input `cell_id` in the
/// model-visible message: `Updated cell ${cell_id} with ${new_source}`.
#[tokio::test]
async fn test_notebook_replace_message_uses_cell_id() {
    let mut notebook = minimal_notebook(5);
    notebook["cells"] = json!([{
        "cell_type": "code",
        "id": "abc-123",
        "source": ["old\n"],
        "metadata": {},
        "execution_count": null,
        "outputs": [],
    }]);
    let file = write_notebook(&notebook);
    let ctx = ToolUseContext::test_default();
    let result = <NotebookEditTool as DynTool>::execute(
        &NotebookEditTool,
        json!({
            "notebook_path": file.path().to_str().unwrap(),
            "cell_id": "abc-123",
            "edit_mode": "replace",
            "new_source": "new code"
        }),
        &ctx,
    )
    .await
    .unwrap();
    let msg = result.data["message"].as_str().unwrap();
    assert_eq!(msg, "Updated cell abc-123 with new code");
}

/// Pre-4.5 notebooks have no cell IDs in the file format. Mirrors the
/// template-literal stringification of `undefined` with the literal
/// string `"undefined"`.
#[tokio::test]
async fn test_notebook_replace_message_emits_undefined_on_pre_45() {
    let mut notebook = minimal_notebook(4);
    notebook["cells"] = json!([{
        "cell_type": "code",
        "source": ["old\n"],
        "metadata": {},
        "execution_count": null,
        "outputs": [],
    }]);
    let file = write_notebook(&notebook);
    let ctx = ToolUseContext::test_default();
    let result = <NotebookEditTool as DynTool>::execute(
        &NotebookEditTool,
        json!({
            "notebook_path": file.path().to_str().unwrap(),
            "cell_id": "0",
            "edit_mode": "replace",
            "new_source": "new"
        }),
        &ctx,
    )
    .await
    .unwrap();
    let msg = result.data["message"].as_str().unwrap();
    assert_eq!(msg, "Updated cell undefined with new");
}

/// Insert on a 4.5+ notebook must use the freshly-generated cell ID:
/// `Inserted cell ${cell_id} with ${new_source}`.
#[tokio::test]
async fn test_notebook_insert_message_uses_generated_id() {
    let notebook = minimal_notebook(5);
    let file = write_notebook(&notebook);
    let ctx = ToolUseContext::test_default();
    let result = <NotebookEditTool as DynTool>::execute(
        &NotebookEditTool,
        json!({
            "notebook_path": file.path().to_str().unwrap(),
            "cell_id": "0",
            "edit_mode": "insert",
            "cell_type": "code",
            "new_source": "x = 1"
        }),
        &ctx,
    )
    .await
    .unwrap();
    let msg = result.data["message"].as_str().unwrap();
    let new_id = result.data["new_cell_id"].as_str().unwrap();
    assert_eq!(msg, format!("Inserted cell {new_id} with x = 1"));
}

/// Delete on a 4.5+ notebook must echo the input `cell_id`:
/// `Deleted cell ${cell_id}`.
#[tokio::test]
async fn test_notebook_delete_message_uses_cell_id() {
    let mut notebook = minimal_notebook(5);
    notebook["cells"] = json!([{
        "cell_type": "code",
        "id": "doomed-cell",
        "source": ["x\n"],
        "metadata": {},
        "execution_count": null,
        "outputs": [],
    }]);
    let file = write_notebook(&notebook);
    let ctx = ToolUseContext::test_default();
    let result = <NotebookEditTool as DynTool>::execute(
        &NotebookEditTool,
        json!({
            "notebook_path": file.path().to_str().unwrap(),
            "cell_id": "doomed-cell",
            "new_source": "",
            "edit_mode": "delete"
        }),
        &ctx,
    )
    .await
    .unwrap();
    let msg = result.data["message"].as_str().unwrap();
    assert_eq!(msg, "Deleted cell doomed-cell");
}

// ---------------------------------------------------------------------------
// #19 — replace one-past-end → insert; cell_type switch on replace
// ---------------------------------------------------------------------------

/// Replacing at index == cells.len() is an append-insert
/// (cell_type defaults to code).
#[tokio::test]
async fn test_notebook_replace_one_past_end_converts_to_insert() {
    let file = write_notebook(&minimal_notebook(5)); // 2 cells (idx 0,1)
    let ctx = ToolUseContext::test_default();
    <NotebookEditTool as DynTool>::execute(
        &NotebookEditTool,
        json!({
            "notebook_path": file.path().to_str().unwrap(),
            "cell_id": "2", // one past the end
            "edit_mode": "replace",
            "new_source": "APPENDED"
        }),
        &ctx,
    )
    .await
    .unwrap();
    let updated: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(file.path()).unwrap()).unwrap();
    let cells = updated["cells"].as_array().unwrap();
    assert_eq!(cells.len(), 3, "one-past-end replace appends a cell");
    assert_eq!(
        cells[2]["cell_type"], "code",
        "appended cell defaults to code"
    );
    assert_eq!(cells[2]["source"], json!(["APPENDED"]));
}

/// A replace whose `cell_type` differs from the target cell switches the
/// cell's type.
#[tokio::test]
async fn test_notebook_replace_applies_cell_type_switch() {
    let file = write_notebook(&minimal_notebook(5));
    let ctx = ToolUseContext::test_default();
    <NotebookEditTool as DynTool>::execute(
        &NotebookEditTool,
        json!({
            "notebook_path": file.path().to_str().unwrap(),
            "cell_id": "0", // a code cell
            "edit_mode": "replace",
            "cell_type": "markdown",
            "new_source": "# now markdown"
        }),
        &ctx,
    )
    .await
    .unwrap();
    let updated: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(file.path()).unwrap()).unwrap();
    assert_eq!(updated["cells"][0]["cell_type"], "markdown");
}

// ---------------------------------------------------------------------------
// #20 / #29 — validate_input (extension guard + insert cell_type)
// ---------------------------------------------------------------------------

#[test]
fn test_notebook_validate_rejects_non_ipynb() {
    let ctx = ToolUseContext::test_default();
    let res = <NotebookEditTool as DynTool>::validate_input(
        &NotebookEditTool,
        &json!({"notebook_path": "/work/file.py", "cell_id": "0", "new_source": "x"}),
        &ctx,
    );
    match res {
        coco_tool_runtime::ValidationResult::Invalid {
            error_code,
            message,
        } => {
            assert_eq!(error_code.as_deref(), Some("2"));
            assert!(message.contains("Jupyter notebook"), "got: {message}");
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn test_notebook_validate_insert_requires_cell_type() {
    let ctx = ToolUseContext::test_default();
    let res = <NotebookEditTool as DynTool>::validate_input(
        &NotebookEditTool,
        &json!({
            "notebook_path": "/work/nb.ipynb",
            "cell_id": "0",
            "edit_mode": "insert",
            "new_source": "x"
        }),
        &ctx,
    );
    match res {
        coco_tool_runtime::ValidationResult::Invalid { error_code, .. } => {
            assert_eq!(error_code.as_deref(), Some("5"));
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
}
