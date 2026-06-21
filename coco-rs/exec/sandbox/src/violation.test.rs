use std::time::SystemTime;

use super::*;

fn make_violation(operation: &str, benign: bool) -> Violation {
    Violation {
        timestamp: SystemTime::now(),
        operation: operation.to_string(),
        path: None,
        command_tag: None,
        benign,
    }
}

fn make_tagged_violation(tag: &str) -> Violation {
    Violation {
        timestamp: SystemTime::now(),
        operation: "file-write-data".to_string(),
        path: Some("/tmp/test".to_string()),
        command_tag: Some(tag.to_string()),
        benign: false,
    }
}

#[test]
fn test_violation_store_new() {
    let store = ViolationStore::new();
    assert_eq!(store.count(), 0);
    assert_eq!(store.total_count(), 0);
}

#[test]
fn test_violation_store_push_and_count() {
    let mut store = ViolationStore::new();
    store.push(make_violation("file-write-data", false));
    store.push(make_violation("network-outbound", false));
    assert_eq!(store.count(), 2);
    assert_eq!(store.total_count(), 2);
}

#[test]
fn test_violation_store_ring_buffer_eviction() {
    let mut store = ViolationStore::with_max_size(3);
    for i in 0..5 {
        store.push(make_violation(&format!("op-{i}"), false));
    }
    // Only last 3 should remain
    assert_eq!(store.count(), 3);
    assert_eq!(store.total_count(), 5);
    let recent = store.recent(3);
    assert_eq!(recent[0].operation, "op-4");
    assert_eq!(recent[1].operation, "op-3");
    assert_eq!(recent[2].operation, "op-2");
}

#[test]
fn test_violation_store_non_benign_count() {
    let mut store = ViolationStore::new();
    store.push(make_violation("file-write-data", false));
    store.push(make_violation("mDNSResponder lookup", true));
    store.push(make_violation("network-outbound", false));
    assert_eq!(store.non_benign_count(), 2);
}

#[test]
fn test_violation_store_recent() {
    let mut store = ViolationStore::new();
    store.push(make_violation("first", false));
    store.push(make_violation("second", false));
    store.push(make_violation("third", false));
    let recent = store.recent(2);
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].operation, "third");
    assert_eq!(recent[1].operation, "second");
}

#[test]
fn test_violation_store_for_command() {
    let mut store = ViolationStore::new();
    store.push(make_tagged_violation("cmd-1"));
    store.push(make_tagged_violation("cmd-2"));
    store.push(make_tagged_violation("cmd-1"));
    let cmd1 = store.for_command("cmd-1");
    assert_eq!(cmd1.len(), 2);
    let cmd2 = store.for_command("cmd-2");
    assert_eq!(cmd2.len(), 1);
    let cmd3 = store.for_command("cmd-3");
    assert_eq!(cmd3.len(), 0);
}

#[test]
fn test_violation_store_clear() {
    let mut store = ViolationStore::new();
    store.push(make_violation("op", false));
    store.push(make_violation("op", false));
    store.clear();
    assert_eq!(store.count(), 0);
    // total_count is preserved
    assert_eq!(store.total_count(), 2);
}

#[test]
fn test_violation_is_benign_pattern() {
    let v1 = Violation {
        timestamp: SystemTime::now(),
        operation: "mDNSResponder lookup denied".to_string(),
        path: None,
        command_tag: None,
        benign: false,
    };
    assert!(v1.is_benign_pattern());

    let v2 = Violation {
        timestamp: SystemTime::now(),
        operation: "file-write-data".to_string(),
        path: None,
        command_tag: None,
        benign: false,
    };
    assert!(!v2.is_benign_pattern());
}

// ==========================================================================
// Observer pattern
// ==========================================================================

#[test]
fn test_observer_notified_on_non_benign_push() {
    let (mut store, mut rx) = ViolationStore::with_observer();

    store.push(Violation {
        timestamp: SystemTime::now(),
        operation: "file-write-data".to_string(),
        path: None,
        command_tag: None,
        benign: false,
    });

    // Should receive notification with non-benign count = 1
    let count = rx.try_recv().expect("should receive notification");
    assert_eq!(count, 1);
}

#[test]
fn test_observer_not_notified_on_benign_push() {
    let (mut store, mut rx) = ViolationStore::with_observer();

    store.push(Violation {
        timestamp: SystemTime::now(),
        operation: "mDNSResponder".to_string(),
        path: None,
        command_tag: None,
        benign: true,
    });

    // Benign violations should not trigger notification
    assert!(rx.try_recv().is_err());
}

#[test]
fn test_observer_delta_counts() {
    let (mut store, mut rx) = ViolationStore::with_observer();

    for i in 0..3 {
        store.push(Violation {
            timestamp: SystemTime::now(),
            operation: format!("deny-{i}"),
            path: None,
            command_tag: None,
            benign: false,
        });
    }

    assert_eq!(rx.try_recv().unwrap(), 1);
    assert_eq!(rx.try_recv().unwrap(), 2);
    assert_eq!(rx.try_recv().unwrap(), 3);
}

#[test]
fn test_ignore_violations_global_pattern() {
    let mut store = ViolationStore::new();
    store.set_ignore_patterns(HashMap::from([(
        "*".to_string(),
        vec!["file-write-data".to_string()],
    )]));

    store.push(make_violation("file-write-data", false));
    assert_eq!(store.count(), 0, "ignored violation should not be stored");

    store.push(make_violation("network-outbound", false));
    assert_eq!(store.count(), 1, "non-matching violation should be stored");
}

#[test]
fn test_ignore_violations_command_specific_pattern() {
    let mut store = ViolationStore::new();
    store.set_ignore_patterns(HashMap::from([(
        "npm".to_string(),
        vec!["mach-lookup".to_string()],
    )]));

    // Violation with matching command tag
    store.push(Violation {
        timestamp: SystemTime::now(),
        operation: "mach-lookup com.apple.CoreSimulator".to_string(),
        path: None,
        command_tag: Some("npm install".to_string()),
        benign: false,
    });
    assert_eq!(
        store.count(),
        0,
        "command-matched violation should be ignored"
    );

    // Same operation but different command
    store.push(Violation {
        timestamp: SystemTime::now(),
        operation: "mach-lookup com.apple.CoreSimulator".to_string(),
        path: None,
        command_tag: Some("git status".to_string()),
        benign: false,
    });
    assert_eq!(
        store.count(),
        1,
        "non-matching command should NOT be ignored"
    );
}

#[test]
fn test_ignore_violations_empty_patterns_no_effect() {
    let mut store = ViolationStore::new();
    store.set_ignore_patterns(HashMap::new());

    store.push(make_violation("file-write-data", false));
    assert_eq!(
        store.count(),
        1,
        "empty patterns should not filter anything"
    );
}

#[test]
fn test_ignore_violations_does_not_affect_total_count() {
    let mut store = ViolationStore::new();
    store.set_ignore_patterns(HashMap::from([(
        "*".to_string(),
        vec!["file-write-data".to_string()],
    )]));

    store.push(make_violation("file-write-data", false));
    // Ignored violations are silently dropped, not counted
    assert_eq!(store.total_count(), 0);
    assert_eq!(store.count(), 0);
}
