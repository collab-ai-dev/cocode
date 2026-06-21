use super::*;
use std::path::PathBuf;

#[test]
fn test_root_watcher_creation() {
    let watcher = RootWatcher::new();
    // Should succeed on platforms with filesystem notification support
    assert!(watcher.is_some());
}

#[test]
fn test_track_and_untrack_single_root() {
    let watcher = RootWatcher::new().expect("watcher should initialize");
    let root = PathBuf::from("/tmp/test-worktree-a");

    watcher.track_root(&root);
    {
        let guard = lock_or_recover(&watcher.parent_to_roots);
        let parent = PathBuf::from("/tmp");
        assert!(guard.contains_key(&parent));
        assert!(guard[&parent].contains(&root));
    }

    watcher.untrack_root(&root);
    {
        let guard = lock_or_recover(&watcher.parent_to_roots);
        let parent = PathBuf::from("/tmp");
        assert!(
            !guard.contains_key(&parent),
            "parent should be removed when last root is untracked"
        );
    }
}

#[test]
fn test_multiple_roots_same_parent() {
    let watcher = RootWatcher::new().expect("watcher should initialize");
    let root_a = PathBuf::from("/tmp/worktrees/feat-a");
    let root_b = PathBuf::from("/tmp/worktrees/feat-b");
    let parent = PathBuf::from("/tmp/worktrees");

    watcher.track_root(&root_a);
    watcher.track_root(&root_b);
    {
        let guard = lock_or_recover(&watcher.parent_to_roots);
        assert_eq!(guard[&parent].len(), 2);
        assert!(guard[&parent].contains(&root_a));
        assert!(guard[&parent].contains(&root_b));
    }

    // Untrack first root — parent should still be watched
    watcher.untrack_root(&root_a);
    {
        let guard = lock_or_recover(&watcher.parent_to_roots);
        assert!(
            guard.contains_key(&parent),
            "parent should remain while roots exist"
        );
        assert_eq!(guard[&parent].len(), 1);
        assert!(guard[&parent].contains(&root_b));
    }

    // Untrack second root — parent should be removed
    watcher.untrack_root(&root_b);
    {
        let guard = lock_or_recover(&watcher.parent_to_roots);
        assert!(!guard.contains_key(&parent));
    }
}

#[test]
fn test_track_root_without_parent() {
    let watcher = RootWatcher::new().expect("watcher should initialize");
    // Root path "/" has no parent — should not panic
    watcher.track_root(Path::new("/"));
    let guard = lock_or_recover(&watcher.parent_to_roots);
    assert!(guard.is_empty(), "root without parent should be skipped");
}

#[test]
fn test_untrack_nonexistent_root() {
    let watcher = RootWatcher::new().expect("watcher should initialize");
    // Untracking a root that was never tracked should not panic
    watcher.untrack_root(Path::new("/tmp/never-tracked"));
    let guard = lock_or_recover(&watcher.parent_to_roots);
    assert!(guard.is_empty());
}

#[test]
fn test_subscribe_returns_receiver() {
    let watcher = RootWatcher::new().expect("watcher should initialize");
    let _rx = watcher.subscribe();
    // Should not panic — receiver is valid
}

#[test]
fn test_merge_events_deduplicates() {
    let a = RootDeleted {
        roots: vec![PathBuf::from("/a"), PathBuf::from("/b")],
    };
    let b = RootDeleted {
        roots: vec![PathBuf::from("/b"), PathBuf::from("/c")],
    };
    let merged = merge_events(a, b);
    assert_eq!(merged.roots.len(), 3);
    assert!(merged.roots.contains(&PathBuf::from("/a")));
    assert!(merged.roots.contains(&PathBuf::from("/b")));
    assert!(merged.roots.contains(&PathBuf::from("/c")));
}

#[test]
fn test_classify_ignores_non_remove_events() {
    let map: Mutex<HashMap<PathBuf, HashSet<PathBuf>>> = Mutex::new(HashMap::new());
    let event = notify::Event {
        kind: EventKind::Modify(notify::event::ModifyKind::Data(
            notify::event::DataChange::Content,
        )),
        paths: vec![PathBuf::from("/tmp/some-root")],
        attrs: Default::default(),
    };
    assert!(classify_event(&event, &map).is_none());
}
