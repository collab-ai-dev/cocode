use super::*;

#[test]
fn test_parse_session_metadata() {
    let json = serde_json::json!({
        "model": "claude-sonnet-4-20250514",
        "working_dir": "/home/user/project",
        "messages": [
            {"role": "user", "content": "hello"},
            {"role": "assistant", "content": "hi"},
            {"role": "user", "content": "help"},
        ]
    });
    let (model, wd, count) = parse_session_metadata(&serde_json::to_string(&json).unwrap());
    assert_eq!(model.as_deref(), Some("claude-sonnet-4-20250514"));
    assert_eq!(wd.as_deref(), Some("/home/user/project"));
    assert_eq!(count, 3);
}

#[test]
fn test_parse_session_metadata_empty() {
    let (model, wd, count) = parse_session_metadata("{}");
    assert!(model.is_none());
    assert!(wd.is_none());
    assert_eq!(count, 0);
}

#[test]
fn test_parse_session_metadata_invalid_json() {
    let (model, wd, count) = parse_session_metadata("not json");
    assert!(model.is_none());
    assert!(wd.is_none());
    assert_eq!(count, 0);
}

#[test]
fn test_format_age() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    assert_eq!(format_age(now), "just now");
    assert_eq!(format_age(now - 120), "2m ago");
    assert_eq!(format_age(now - 7200), "2h ago");
    assert_eq!(format_age(now - 172800), "2d ago");
}

#[test]
fn test_truncate_id_short() {
    assert_eq!(truncate_id("abc", 10), "abc");
}

#[test]
fn test_truncate_id_long() {
    let long = "abcdefghijklmnopqrstuvwxyz";
    let result = truncate_id(long, 10);
    assert_eq!(result.len(), 10);
    assert!(result.ends_with("..."));
}

#[tokio::test]
async fn test_list_sessions_empty_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let result = list_sessions(tmp.path()).await.unwrap();
    assert!(result.contains("No session files found"));
}

#[tokio::test]
async fn test_list_sessions_with_files() {
    let tmp = tempfile::tempdir().unwrap();

    for i in 0..3 {
        let session = serde_json::json!({
            "model": "claude-sonnet-4-20250514",
            "messages": [{"role": "user", "content": format!("message {i}")}],
        });
        tokio::fs::write(
            tmp.path().join(format!("session-{i}.json")),
            serde_json::to_string(&session).unwrap(),
        )
        .await
        .unwrap();
    }

    let result = list_sessions(tmp.path()).await.unwrap();
    assert!(result.contains("3 found"));
    assert!(result.contains("session-0"));
    assert!(result.contains("session-1"));
    assert!(result.contains("session-2"));
}

#[tokio::test]
async fn test_delete_session() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("my-session.json");
    tokio::fs::write(&path, "{}").await.unwrap();

    let result = delete_session(tmp.path(), "my-session").await.unwrap();
    assert!(result.contains("Deleted session"));
    assert!(!path.exists());
}

#[tokio::test]
async fn test_delete_session_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let result = delete_session(tmp.path(), "nonexistent").await.unwrap();
    assert!(result.contains("No session matching"));
}

#[tokio::test]
async fn test_session_info() {
    let tmp = tempfile::tempdir().unwrap();
    let session = serde_json::json!({
        "model": "claude-opus-4-20250514",
        "working_dir": "/tmp/test",
        "messages": [
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": "hello"},
        ]
    });
    tokio::fs::write(
        tmp.path().join("test-sess.json"),
        serde_json::to_string(&session).unwrap(),
    )
    .await
    .unwrap();

    let result = session_info(tmp.path(), "test-sess").await.unwrap();
    assert!(result.contains("test-sess"));
    assert!(result.contains("opus"));
    assert!(result.contains("Messages:    2"));
}

#[tokio::test]
async fn test_handler_list_subcommand() {
    let output = handler("list".to_string()).await.unwrap();
    // Should either show sessions or say none found
    assert!(
        output.contains("Sessions") || output.contains("No session"),
        "unexpected: {output}"
    );
}
