use super::*;

#[test]
fn drain_returns_pushed_in_fifo_order_and_clears() {
    let inbox = NoticeInbox::new();
    inbox.push(MemoryUserNotice {
        written_paths: vec!["a.md".into()],
        verb: NoticeVerb::Saved,
    });
    inbox.push(MemoryUserNotice {
        written_paths: vec!["b.md".into(), "c.md".into()],
        verb: NoticeVerb::Improved,
    });
    assert_eq!(inbox.len(), 2);
    let drained = inbox.drain();
    assert_eq!(drained.len(), 2);
    assert_eq!(drained[0].written_paths, vec!["a.md".to_string()]);
    assert_eq!(drained[0].verb, NoticeVerb::Saved);
    assert_eq!(drained[1].verb, NoticeVerb::Improved);
    // Drain clears.
    assert_eq!(inbox.len(), 0);
    assert!(inbox.drain().is_empty());
}

#[test]
fn verb_str_round_trips() {
    assert_eq!(NoticeVerb::Saved.as_str(), "Saved");
    assert_eq!(NoticeVerb::Improved.as_str(), "Improved");
}

#[test]
fn clones_share_storage() {
    let a = NoticeInbox::new();
    let b = a.clone();
    a.push(MemoryUserNotice {
        written_paths: vec!["x.md".into()],
        verb: NoticeVerb::Saved,
    });
    // Either handle drains the same underlying buffer.
    let drained = b.drain();
    assert_eq!(drained.len(), 1);
    assert_eq!(a.len(), 0);
}

#[test]
fn cross_source_dedup_picks_highest_priority_verb() {
    // When the same path is reported by multiple sources in one turn,
    // the higher-priority verb wins: Saved (3) > Improved (2) > ManualEdit (1).
    let inbox = NoticeInbox::new();
    // ManualEdit on foo.md (engine post-write classification).
    inbox.push(MemoryUserNotice {
        written_paths: vec!["foo.md".into(), "baz.md".into()],
        verb: NoticeVerb::ManualEdit,
    });
    // Improved on foo.md + bar.md (dream).
    inbox.push(MemoryUserNotice {
        written_paths: vec!["foo.md".into(), "bar.md".into()],
        verb: NoticeVerb::Improved,
    });
    // Saved on foo.md (extract).
    inbox.push(MemoryUserNotice {
        written_paths: vec!["foo.md".into()],
        verb: NoticeVerb::Saved,
    });

    let drained = inbox.drain();
    // Expect three groups (one per verb that won at least once),
    // in priority order: Saved, Improved, ManualEdit.
    assert_eq!(drained.len(), 3);
    assert_eq!(drained[0].verb, NoticeVerb::Saved);
    assert_eq!(drained[0].written_paths, vec!["foo.md".to_string()]);
    assert_eq!(drained[1].verb, NoticeVerb::Improved);
    assert_eq!(drained[1].written_paths, vec!["bar.md".to_string()]);
    assert_eq!(drained[2].verb, NoticeVerb::ManualEdit);
    assert_eq!(drained[2].written_paths, vec!["baz.md".to_string()]);
}

#[test]
fn cross_source_dedup_collapses_duplicate_paths_within_same_verb() {
    // Same path repeated under the same verb across multiple pushes
    // should collapse to a single entry.
    let inbox = NoticeInbox::new();
    inbox.push(MemoryUserNotice {
        written_paths: vec!["a.md".into(), "b.md".into()],
        verb: NoticeVerb::Saved,
    });
    inbox.push(MemoryUserNotice {
        written_paths: vec!["b.md".into(), "c.md".into()],
        verb: NoticeVerb::Saved,
    });
    let drained = inbox.drain();
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].verb, NoticeVerb::Saved);
    assert_eq!(
        drained[0].written_paths,
        vec!["a.md".to_string(), "b.md".to_string(), "c.md".to_string()]
    );
}
