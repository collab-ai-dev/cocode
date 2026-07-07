use serde_json::json;

use super::*;

fn test_session_id(value: &str) -> SessionId {
    match SessionId::try_new(value) {
        Ok(id) => id,
        Err(_) => unreachable!("test session id should be valid"),
    }
}

#[test]
fn persisted_worktree_session_session_id_wire_stays_string() {
    let entry = PersistedWorktreeSession {
        original_cwd: "/repo".into(),
        worktree_path: "/repo-wt".into(),
        worktree_name: "feature".into(),
        worktree_branch: Some("feature".into()),
        original_branch: Some("main".into()),
        original_head_commit: Some("abc123".into()),
        session_id: test_session_id("session-1"),
        tmux_session_name: None,
        hook_based: false,
    };

    let value = serde_json::to_value(&entry).expect("serialize worktree session");
    assert_eq!(value["session_id"], "session-1");

    let decoded: PersistedWorktreeSession = serde_json::from_value(json!({
        "original_cwd": "/repo",
        "worktree_path": "/repo-wt",
        "worktree_name": "feature",
        "session_id": "session-1"
    }))
    .expect("deserialize worktree session");
    assert_eq!(decoded.session_id.as_str(), "session-1");
}
