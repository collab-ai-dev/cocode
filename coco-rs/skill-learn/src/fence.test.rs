use std::path::PathBuf;
use std::sync::Arc;

use coco_tool_runtime::{CanUseToolCallContext, CanUseToolDecision, TurnAbortSignal};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::create_skill_write_handle;

const ROOT: &str = "/agent";

fn ctx() -> CanUseToolCallContext {
    CanUseToolCallContext {
        tool_use_id: "test".into(),
        cwd: PathBuf::from(ROOT),
        abort: TurnAbortSignal::from_token(CancellationToken::new()),
        require_can_use_tool: false,
        messages: Arc::new(Vec::new()),
    }
}

fn assert_allowed(d: &CanUseToolDecision, msg: &str) {
    assert!(
        matches!(d, CanUseToolDecision::Allow { .. }),
        "expected Allow for {msg}, got {d:?}"
    );
}

fn assert_denied(d: &CanUseToolDecision, msg: &str) {
    assert!(
        matches!(d, CanUseToolDecision::Deny { .. }),
        "expected Deny for {msg}, got {d:?}"
    );
}

fn handle() -> coco_tool_runtime::CanUseToolHandleRef {
    create_skill_write_handle(PathBuf::from(ROOT))
}

#[tokio::test]
async fn allows_read_glob_grep() {
    let h = handle();
    for tool in ["Read", "Glob", "Grep"] {
        let d = h.check(tool, &json!({}), &ctx()).await;
        assert_allowed(&d, tool);
    }
}

#[tokio::test]
async fn allows_read_only_bash() {
    let h = handle();
    let d = h
        .check("Bash", &json!({"command": "git status"}), &ctx())
        .await;
    assert_allowed(&d, "read-only bash");
}

#[tokio::test]
async fn denies_mutating_bash() {
    let h = handle();
    let d = h
        .check("Bash", &json!({"command": "rm -rf /"}), &ctx())
        .await;
    assert_denied(&d, "mutating bash");
}

#[tokio::test]
async fn denies_redirection_bash() {
    let h = handle();
    let d = h
        .check("Bash", &json!({"command": "echo x > /agent/f"}), &ctx())
        .await;
    assert_denied(&d, "redirection bash");
}

#[tokio::test]
async fn allows_write_under_root() {
    let h = handle();
    let d = h
        .check(
            "Write",
            &json!({"file_path": "/agent/my-skill/SKILL.md"}),
            &ctx(),
        )
        .await;
    assert_allowed(&d, "write under agent root");
}

#[tokio::test]
async fn allows_edit_relative_resolved_against_cwd() {
    let h = handle();
    let d = h
        .check("Edit", &json!({"file_path": "my-skill/SKILL.md"}), &ctx())
        .await;
    assert_allowed(&d, "relative edit under cwd (agent root)");
}

#[tokio::test]
async fn denies_write_outside_root() {
    let h = handle();
    let d = h
        .check("Write", &json!({"file_path": "/etc/passwd"}), &ctx())
        .await;
    assert_denied(&d, "write outside agent root");
}

#[tokio::test]
async fn denies_hidden_path_components() {
    let h = handle();
    // A dotfile inside the root is invisible to discovery and could collide
    // with loop metadata — the filename policy rejects it even though
    // containment passes.
    let d = h
        .check(
            "Write",
            &json!({"file_path": "/agent/.sneaky/SKILL.md"}),
            &ctx(),
        )
        .await;
    assert_denied(&d, "hidden directory component");
    let d = h
        .check(
            "Write",
            &json!({"file_path": "/agent/my-skill/.lock.md"}),
            &ctx(),
        )
        .await;
    assert_denied(&d, "hidden file");
}

#[tokio::test]
async fn denies_disallowed_extensions() {
    let h = handle();
    for path in [
        "/agent/my-skill/run.sh",
        "/agent/my-skill/payload.py",
        "/agent/my-skill/SKILL", // no extension
    ] {
        let d = h.check("Write", &json!({"file_path": path}), &ctx()).await;
        assert_denied(&d, path);
    }
    // Documentation-class support files stay allowed.
    let d = h
        .check(
            "Write",
            &json!({"file_path": "/agent/my-skill/notes.txt"}),
            &ctx(),
        )
        .await;
    assert_allowed(&d, "txt support file");
}

#[tokio::test]
async fn denies_traversal_escape() {
    let h = handle();
    let d = h
        .check(
            "Edit",
            &json!({"file_path": "/agent/../etc/passwd"}),
            &ctx(),
        )
        .await;
    assert_denied(&d, "traversal escape");
}

#[tokio::test]
async fn allows_apply_patch_under_root() {
    let h = handle();
    let patch = "*** Begin Patch\n*** Add File: my-skill/SKILL.md\n+hello\n*** End Patch\n";
    let d = h
        .check("apply_patch", &json!({"patch": patch}), &ctx())
        .await;
    assert_allowed(&d, "apply_patch under root");
}

#[tokio::test]
async fn denies_apply_patch_escape() {
    let h = handle();
    let patch = "*** Begin Patch\n*** Add File: ok.md\n+hello\n*** Add File: ../outside.md\n+bad\n*** End Patch\n";
    let d = h
        .check("apply_patch", &json!({"patch": patch}), &ctx())
        .await;
    assert_denied(&d, "apply_patch escape");
}

#[tokio::test]
async fn denies_unknown_tool() {
    let h = handle();
    let d = h.check("WebFetch", &json!({}), &ctx()).await;
    assert_denied(&d, "unknown tool");
}
