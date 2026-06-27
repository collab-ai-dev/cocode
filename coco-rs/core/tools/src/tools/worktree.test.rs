//! Tests for EnterWorktreeTool and ExitWorktreeTool.

use super::EnterWorktreeTool;
use super::ExitWorktreeTool;
use coco_tool_runtime::DynTool;
use coco_tool_runtime::ToolUseContext;
use coco_types::ActiveWorktreeState;
use coco_types::ToolAppState;
use serde_json::json;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

struct CwdGuard(PathBuf);

impl CwdGuard {
    fn set(path: &Path) -> Self {
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(path).unwrap();
        Self(previous)
    }
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.0);
    }
}

// ---------------------------------------------------------------------------
// Input validation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_exit_worktree_rejects_missing_state() {
    let ctx = ToolUseContext::test_default();
    let result =
        <ExitWorktreeTool as DynTool>::execute(&ExitWorktreeTool, json!({"action": "keep"}), &ctx)
            .await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("active worktree"),
        "should mention state: {err}"
    );
}

#[tokio::test]
async fn test_exit_worktree_rejects_when_no_active_worktree() {
    let mut ctx = ToolUseContext::test_default();
    ctx.app_state = Some(Arc::new(RwLock::new(ToolAppState::default())).into());
    let result =
        <ExitWorktreeTool as DynTool>::execute(&ExitWorktreeTool, json!({"action": "keep"}), &ctx)
            .await;
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("No active worktree")
    );
}

#[tokio::test]
async fn test_enter_worktree_rejects_missing_app_state_before_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    // `current_dir()` returns the realpath; on macOS `tempfile` hands back the
    // `/var` symlink, so canonicalize the expected path or the asserts below
    // compare `/private/var/...` against `/var/...`.
    let repo = std::fs::canonicalize(&repo).unwrap();
    let session_cwd = Arc::new(RwLock::new(repo.clone()));
    let _cwd_guard = CwdGuard::set(&repo);

    let mut ctx = ToolUseContext::test_default();
    ctx.session_cwd = Some(session_cwd.clone());
    ctx.app_state = None;

    let result = <EnterWorktreeTool as DynTool>::execute(
        &EnterWorktreeTool,
        json!({"name": "missing-state"}),
        &ctx,
    )
    .await;

    assert!(result.is_err());
    // Canonicalize both sides: on macOS the tempdir lives under /var which is a
    // symlink to /private/var, so current_dir() resolves while `repo` does not.
    assert_eq!(
        std::env::current_dir().unwrap().canonicalize().unwrap(),
        repo.canonicalize().unwrap()
    );
    assert_eq!(*session_cwd.read().await, repo);
    assert!(!temp.path().join("worktrees").join("missing-state").exists());
}

#[test]
fn test_enter_worktree_schema_uses_name_only() {
    let schema =
        <EnterWorktreeTool as DynTool>::runtime_validation_schema(&EnterWorktreeTool).as_value();
    assert!(schema["properties"].get("name").is_some());
    assert!(schema["properties"].get("branch").is_none());
    assert!(schema["properties"].get("path").is_none());
}

#[test]
fn test_exit_worktree_schema_uses_action_not_path() {
    let schema =
        <ExitWorktreeTool as DynTool>::runtime_validation_schema(&ExitWorktreeTool).as_value();
    assert!(schema["properties"].get("action").is_some());
    assert!(schema["properties"].get("discard_changes").is_some());
    assert!(schema["properties"].get("path").is_none());
    assert!(schema["properties"].get("previous_cwd").is_none());
}

#[tokio::test]
async fn test_exit_worktree_refuses_unverifiable_without_discard() {
    let temp = tempfile::tempdir().unwrap();
    let original = temp.path().join("repo");
    std::fs::create_dir_all(&original).unwrap();
    let worktree = temp.path().join("missing-worktree");
    let app_state = Arc::new(RwLock::new(ToolAppState {
        active_worktree: Some(ActiveWorktreeState {
            original_cwd: original,
            worktree_path: worktree,
            worktree_branch: Some("agent/task-test".into()),
            original_head_commit: None,
        }),
        ..ToolAppState::default()
    }));
    let mut ctx = ToolUseContext::test_default();
    ctx.app_state = Some(app_state.into());
    let result = <ExitWorktreeTool as DynTool>::execute(
        &ExitWorktreeTool,
        json!({ "action": "remove" }),
        &ctx,
    )
    .await;
    let err = result.expect_err("should refuse").to_string();
    assert!(
        err.contains("discard_changes"),
        "refusal must tell the model how to confirm: {err}"
    );
}

#[tokio::test]
async fn test_exit_worktree_keep_restores_cwd_and_clears_state() {
    let temp = tempfile::tempdir().unwrap();
    let original = temp.path().join("repo");
    let worktree = temp.path().join("worktree");
    std::fs::create_dir_all(&original).unwrap();
    std::fs::create_dir_all(&worktree).unwrap();
    // Canonicalize so the cwd asserts compare realpaths (macOS `tempfile`
    // yields the `/var` -> `/private/var` symlink that `current_dir()` resolves).
    let original = std::fs::canonicalize(&original).unwrap();
    let worktree = std::fs::canonicalize(&worktree).unwrap();
    let app_state = Arc::new(RwLock::new(ToolAppState {
        active_worktree: Some(ActiveWorktreeState {
            original_cwd: original.clone(),
            worktree_path: worktree.clone(),
            worktree_branch: Some("agent/task-test".into()),
            original_head_commit: None,
        }),
        ..ToolAppState::default()
    }));
    let session_cwd = Arc::new(RwLock::new(worktree.clone()));
    let _cwd_guard = CwdGuard::set(&worktree);

    let mut ctx = ToolUseContext::test_default();
    ctx.app_state = Some(app_state.clone().into());
    ctx.session_cwd = Some(session_cwd.clone());

    let mut result =
        <ExitWorktreeTool as DynTool>::execute(&ExitWorktreeTool, json!({"action": "keep"}), &ctx)
            .await
            .unwrap();
    if let Some(patch) = result.app_state_patch.take() {
        let mut app_state = app_state.write().await;
        patch(&mut app_state);
    }

    // Canonicalize: macOS /var -> /private/var symlink (see note above).
    assert_eq!(
        std::env::current_dir().unwrap().canonicalize().unwrap(),
        original.canonicalize().unwrap()
    );
    assert_eq!(*session_cwd.read().await, original);
    assert!(worktree.exists(), "keep must not remove worktree dir");
    assert!(app_state.read().await.active_worktree.is_none());
    assert_eq!(result.data["worktreePath"], worktree.display().to_string());
    assert_eq!(result.data["worktreeBranch"], "agent/task-test");
    // output fields.
    assert_eq!(result.data["action"], "keep");
    assert_eq!(result.data["originalCwd"], original.display().to_string());
    assert!(
        result.data.get("discardedFiles").is_none(),
        "keep discards nothing → field omitted"
    );
    assert!(result.data.get("discardedCommits").is_none());
}

// ---------------------------------------------------------------------------
// render_for_model — both worktree tools surface the prebuilt `message` field
// ---------------------------------------------------------------------------

#[test]
fn enter_worktree_render_emits_message_text_only() {
    use coco_tool_runtime::ToolResultContentPart;
    use serde_json::json;
    let data = json!({
        "message": "Created and entered worktree at '/tmp/wt' on branch 'agent/task-x'",
        "worktreePath": "/tmp/wt",
        "worktreeBranch": "agent/task-x",
    });
    let parts = <EnterWorktreeTool as DynTool>::render_for_model(&EnterWorktreeTool, &data);
    let ToolResultContentPart::Text { text, .. } = &parts[0] else {
        panic!("expected Text part");
    };
    assert_eq!(
        text,
        "Created and entered worktree at '/tmp/wt' on branch 'agent/task-x'"
    );
}

#[test]
fn exit_worktree_render_emits_message_text_only() {
    use coco_tool_runtime::ToolResultContentPart;
    use serde_json::json;
    let data = json!({
        "message": "Exited and removed worktree at '/tmp/wt'",
        "worktreePath": "/tmp/wt",
        "worktreeBranch": "agent/task-x",
    });
    let parts = <ExitWorktreeTool as DynTool>::render_for_model(&ExitWorktreeTool, &data);
    let ToolResultContentPart::Text { text, .. } = &parts[0] else {
        panic!("expected Text part");
    };
    assert_eq!(text, "Exited and removed worktree at '/tmp/wt'");
}
