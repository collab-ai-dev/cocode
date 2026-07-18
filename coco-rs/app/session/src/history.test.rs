use super::*;

fn test_session_id(value: &str) -> coco_types::SessionId {
    match coco_types::SessionId::try_new(value) {
        Ok(id) => id,
        Err(_) => unreachable!("test session id should be valid"),
    }
}

#[test]
fn test_add_and_read_history() {
    let dir = tempfile::tempdir().unwrap();
    let history = PromptHistory::new(dir.path(), "/test/project", &test_session_id("session-1"));

    history.add("first command").unwrap();
    history.add("second command").unwrap();
    history.add("third command").unwrap();

    let entries = history.get_history();
    assert_eq!(entries.len(), 3);
    // Newest first
    assert_eq!(entries[0].display, "third command");
    assert_eq!(entries[1].display, "second command");
    assert_eq!(entries[2].display, "first command");
}

#[test]
fn test_history_filters_by_project() {
    let dir = tempfile::tempdir().unwrap();
    let h1 = PromptHistory::new(dir.path(), "/project/a", &test_session_id("s1"));
    let h2 = PromptHistory::new(dir.path(), "/project/b", &test_session_id("s2"));

    h1.add("command for a").unwrap();
    h2.add("command for b").unwrap();

    let entries_a = h1.get_history();
    assert_eq!(entries_a.len(), 1);
    assert_eq!(entries_a[0].display, "command for a");
}

#[test]
fn test_current_session_first() {
    let dir = tempfile::tempdir().unwrap();
    let h1 = PromptHistory::new(dir.path(), "/project", &test_session_id("session-old"));
    let h2 = PromptHistory::new(dir.path(), "/project", &test_session_id("session-new"));

    h1.add("old session command").unwrap();
    h2.add("new session command").unwrap();

    let entries = h2.get_history();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].display, "new session command");
    assert_eq!(entries[1].display, "old session command");
}

#[test]
fn test_empty_history() {
    let dir = tempfile::tempdir().unwrap();
    let history = PromptHistory::new(dir.path(), "/project", &test_session_id("s1"));
    let entries = history.get_history();
    assert!(entries.is_empty());
}

#[test]
fn test_format_pasted_text_ref() {
    assert_eq!(format_pasted_text_ref(1, 0), "[Pasted text #1]");
    assert_eq!(format_pasted_text_ref(2, 10), "[Pasted text #2 +10 lines]");
}

#[test]
fn timestamped_history_lazily_resolves_paste_payloads() {
    let dir = tempfile::tempdir().unwrap();
    let history = PromptHistory::new(dir.path(), "/project", &test_session_id("session-1"));
    history
        .add_with_pastes(
            "inspect [Pasted text #1]",
            &std::collections::HashMap::from([(1, "payload".to_string())]),
        )
        .unwrap();

    let mut entries = history.get_timestamped_history();
    let entry = entries.pop().expect("timestamped history row");
    assert_eq!(entry.display, "inspect [Pasted text #1]");
    assert!(entry.timestamp > 0);
    let resolved = (entry.resolve)();
    assert_eq!(
        resolved.pasted_contents.get(&1).map(String::as_str),
        Some("payload")
    );
}

#[test]
fn test_format_image_ref() {
    assert_eq!(format_image_ref(3), "[Image #3]");
}
