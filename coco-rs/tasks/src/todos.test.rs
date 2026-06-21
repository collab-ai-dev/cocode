use super::*;
use coco_tool_runtime::check_verification_nudge;

fn item(content: &str, status: &str, active: &str) -> TodoItem {
    TodoItem {
        content: content.into(),
        status: status.into(),
        active_form: active.into(),
    }
}

#[test]
fn test_read_unset_key_is_empty() {
    let store = TodoStore::new();
    assert!(store.read("missing").is_empty());
}

#[test]
fn test_write_then_read_round_trip() {
    let store = TodoStore::new();
    let items = vec![item("a", "pending", "Doing a")];
    store.write("agent-1", items.clone());
    assert_eq!(store.read("agent-1"), items);
}

#[test]
fn test_per_key_isolation() {
    let store = TodoStore::new();
    store.write("agent-1", vec![item("a", "pending", "Doing a")]);
    store.write("agent-2", vec![item("b", "pending", "Doing b")]);
    assert_eq!(store.read("agent-1")[0].content, "a");
    assert_eq!(store.read("agent-2")[0].content, "b");
}

#[test]
fn test_empty_write_removes_key() {
    let store = TodoStore::new();
    store.write("k", vec![item("x", "pending", "Doing x")]);
    store.write("k", Vec::new());
    assert!(store.read("k").is_empty());
}

#[test]
fn test_verification_nudge_positive_case() {
    // 3+ items, none mention verification.
    assert!(check_verification_nudge(&[
        "write code",
        "run tests",
        "fix bug"
    ]));
}

#[test]
fn test_verification_nudge_negative_under_threshold() {
    assert!(!check_verification_nudge(&["a", "b"]));
}

#[test]
fn test_verification_nudge_skipped_when_verify_item_exists() {
    assert!(!check_verification_nudge(&[
        "write code",
        "run tests",
        "Verify output"
    ]));
    // Case-insensitive, substring match on /verif/i.
    assert!(!check_verification_nudge(&[
        "a",
        "b",
        "c",
        "VERIFICATION step"
    ]));
}

#[test]
fn test_is_valid_status() {
    assert!(TodoItem::is_valid_status("pending"));
    assert!(TodoItem::is_valid_status("in_progress"));
    assert!(TodoItem::is_valid_status("completed"));
    assert!(!TodoItem::is_valid_status("deleted"));
    assert!(!TodoItem::is_valid_status(""));
}
