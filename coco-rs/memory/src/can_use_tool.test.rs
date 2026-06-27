use std::path::PathBuf;
use std::sync::Arc;

use coco_tool_runtime::CanUseToolCallContext;
use coco_tool_runtime::CanUseToolDecision;
use coco_tool_runtime::TurnAbortSignal;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::create_auto_mem_handle;
use super::create_session_mem_handle;

fn ctx() -> CanUseToolCallContext {
    CanUseToolCallContext {
        tool_use_id: "test".into(),
        abort: TurnAbortSignal::from_token(CancellationToken::new()),
        require_can_use_tool: false,
        messages: Arc::new(Vec::new()),
    }
}

fn assert_allowed(d: &CanUseToolDecision, msg: &str) {
    matches!(d, CanUseToolDecision::Allow { .. })
        .then_some(())
        .unwrap_or_else(|| panic!("expected Allow for {msg}, got {d:?}"));
}

fn assert_denied(d: &CanUseToolDecision, msg: &str) {
    matches!(d, CanUseToolDecision::Deny { .. })
        .then_some(())
        .unwrap_or_else(|| panic!("expected Deny for {msg}, got {d:?}"));
}

// ── auto_mem tests ─────────────────────────────────────────────

#[tokio::test]
async fn test_auto_mem_allows_read_glob_grep_unrestricted() {
    let h = create_auto_mem_handle(PathBuf::from("/memdir"));
    for tool in ["Read", "Glob", "Grep"] {
        let d = h.check(tool, &json!({}), &ctx()).await;
        assert_allowed(&d, tool);
    }
}

#[tokio::test]
async fn test_auto_mem_allows_read_only_bash() {
    let h = create_auto_mem_handle(PathBuf::from("/memdir"));
    let cases = [
        json!({"command": "ls /tmp"}),
        json!({"command": "cat README.md"}),
        json!({"command": "grep foo bar.txt"}),
    ];
    for cmd in cases {
        let d = h.check("Bash", &cmd, &ctx()).await;
        assert_allowed(&d, &cmd.to_string());
    }
}

#[tokio::test]
async fn test_auto_mem_denies_mutating_bash() {
    let h = create_auto_mem_handle(PathBuf::from("/memdir"));
    let cases = [
        json!({"command": "rm -rf /"}),
        json!({"command": "echo bad > /etc/passwd"}),
        json!({"command": "curl http://evil"}),
    ];
    for cmd in cases {
        let d = h.check("Bash", &cmd, &ctx()).await;
        assert_denied(&d, &cmd.to_string());
    }
}

#[tokio::test]
async fn test_auto_mem_allows_safe_piped_bash() {
    // Pipes joining safe-command stages must pass.
    let h = create_auto_mem_handle(PathBuf::from("/memdir"));
    let cases = [
        json!({"command": "ls /tmp | head -10"}),
        json!({"command": "cat README.md | wc -l"}),
        json!({"command": "git log --oneline | head"}),
    ];
    for cmd in cases {
        let d = h.check("Bash", &cmd, &ctx()).await;
        assert_allowed(&d, &cmd.to_string());
    }
}

#[tokio::test]
async fn test_auto_mem_denies_pipe_to_mutating() {
    // A pipeline mixing safe + mutating must fail closed: the
    // mutating stage taints the whole pipeline.
    let h = create_auto_mem_handle(PathBuf::from("/memdir"));
    let cases = [
        json!({"command": "cat /etc/hosts | tee /tmp/x"}),
        json!({"command": "ls / && rm -rf /tmp"}),
    ];
    for cmd in cases {
        let d = h.check("Bash", &cmd, &ctx()).await;
        assert_denied(&d, &cmd.to_string());
    }
}

#[tokio::test]
async fn test_auto_mem_allows_edit_within_memdir() {
    let h = create_auto_mem_handle(PathBuf::from("/memdir"));
    let d = h
        .check("Edit", &json!({"file_path": "/memdir/notes.md"}), &ctx())
        .await;
    assert_allowed(&d, "edit within memdir");
}

#[tokio::test]
async fn test_auto_mem_denies_edit_outside_memdir() {
    let h = create_auto_mem_handle(PathBuf::from("/memdir"));
    let d = h
        .check("Edit", &json!({"file_path": "/tmp/x"}), &ctx())
        .await;
    assert_denied(&d, "edit outside memdir");
}

#[tokio::test]
async fn test_auto_mem_denies_traversal_escape() {
    // Lexically normalize collapses `..` so the candidate is `/x`,
    // not `/memdir/x` — Deny.
    let h = create_auto_mem_handle(PathBuf::from("/memdir"));
    let d = h
        .check("Edit", &json!({"file_path": "/memdir/../x"}), &ctx())
        .await;
    assert_denied(&d, "traversal escape");
}

#[tokio::test]
async fn test_auto_mem_denies_unknown_tool() {
    let h = create_auto_mem_handle(PathBuf::from("/memdir"));
    let d = h.check("TaskOutput", &json!({}), &ctx()).await;
    assert_denied(&d, "unknown tool");
}

// ── session_mem tests ──────────────────────────────────────────

#[tokio::test]
async fn test_session_mem_allows_read() {
    let h = create_session_mem_handle(PathBuf::from("/sessions/sm.md"));
    let d = h.check("Read", &json!({}), &ctx()).await;
    assert_allowed(&d, "read");
}

#[tokio::test]
async fn test_session_mem_allows_edit_on_exact_path() {
    let h = create_session_mem_handle(PathBuf::from("/sessions/sm.md"));
    let d = h
        .check("Edit", &json!({"file_path": "/sessions/sm.md"}), &ctx())
        .await;
    assert_allowed(&d, "edit on canonical path");
}

#[tokio::test]
async fn test_session_mem_denies_edit_on_wrong_path() {
    let h = create_session_mem_handle(PathBuf::from("/sessions/sm.md"));
    let d = h
        .check("Edit", &json!({"file_path": "/tmp/other.md"}), &ctx())
        .await;
    assert_denied(&d, "edit wrong path");
}

#[tokio::test]
async fn test_session_mem_denies_other_tools() {
    let h = create_session_mem_handle(PathBuf::from("/sessions/sm.md"));
    for tool in ["Write", "Bash", "Glob", "Grep", "TaskOutput"] {
        let d = h.check(tool, &json!({}), &ctx()).await;
        assert_denied(&d, tool);
    }
}

// ── symlink-escape regression tests ────────────────────────────

/// A symlink rooted inside the memdir that resolves OUTSIDE must be
/// caught by the realpath layer. Before the fix, the lexical-only
/// fallback would silently Allow the write.
#[tokio::test]
async fn test_auto_mem_denies_real_symlink_escape() {
    let temp = tempfile::tempdir().unwrap();
    let memdir = temp.path().join("memdir");
    std::fs::create_dir_all(&memdir).unwrap();
    let escape_target = temp.path().join("escape-target");
    std::fs::write(&escape_target, "secret").unwrap();
    let escape_link = memdir.join("escape");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&escape_target, &escape_link).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&escape_target, &escape_link).unwrap();

    let h = create_auto_mem_handle(memdir.clone());
    let d = h
        .check(
            "Edit",
            &json!({"file_path": escape_link.display().to_string()}),
            &ctx(),
        )
        .await;
    assert_denied(
        &d,
        "symlink rooted inside memdir but resolving outside must be Denied",
    );
}

/// A dangling symlink (target doesn't exist) inside the memdir is the
/// case where lexical-only fallback was vulnerable. `realpath` returns
/// `None` (dangling). The asymmetric-fail-closed branch must kick in.
#[tokio::test]
#[cfg(unix)]
async fn test_auto_mem_denies_dangling_symlink_inside_memdir() {
    let temp = tempfile::tempdir().unwrap();
    let memdir = temp.path().join("memdir");
    std::fs::create_dir_all(&memdir).unwrap();
    let dangling = memdir.join("dangling");
    std::os::unix::fs::symlink("/nonexistent/path/that/will/never/exist", &dangling).unwrap();

    let h = create_auto_mem_handle(memdir.clone());
    let d = h
        .check(
            "Edit",
            &json!({"file_path": dangling.display().to_string()}),
            &ctx(),
        )
        .await;
    assert_denied(&d, "dangling symlink inside memdir must fail closed");
}
