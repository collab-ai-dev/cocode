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
    assert_eq!(entries[0].composer.text, "third command");
    assert_eq!(entries[1].composer.text, "second command");
    assert_eq!(entries[2].composer.text, "first command");
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
    assert_eq!(entries_a[0].composer.text, "command for a");
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
    assert_eq!(entries[0].composer.text, "new session command");
    assert_eq!(entries[1].composer.text, "old session command");
}

#[test]
fn test_empty_history() {
    let dir = tempfile::tempdir().unwrap();
    let history = PromptHistory::new(dir.path(), "/project", &test_session_id("s1"));
    let entries = history.get_history();
    assert!(entries.is_empty());
}

#[test]
fn timestamped_history_lazily_resolves_paste_payloads() {
    let dir = tempfile::tempdir().unwrap();
    let history = PromptHistory::new(dir.path(), "/project", &test_session_id("session-1"));
    history
        .add_composer(&coco_types::PersistedComposer {
            text: "inspect [Pasted text #1]".into(),
            next_attachment_label: 1,
            elements: vec![coco_types::PersistedComposerElement::Paste {
                start: 8,
                end: 24,
                content: "payload".into(),
            }],
        })
        .unwrap();

    let mut entries = history.get_timestamped_history();
    let entry = entries.pop().expect("timestamped history row");
    assert_eq!(entry.composer.text, "inspect [Pasted text #1]");
    assert!(entry.timestamp > 0);
    let resolved = (entry.resolve)().expect("stored paste resolves");
    assert!(matches!(
        resolved.composer.elements.as_slice(),
        [coco_types::PersistedComposerElement::Paste { content, .. }]
            if content == "payload"
    ));
}

#[test]
fn large_unicode_paste_roundtrips_through_verified_blob_storage() {
    let dir = tempfile::tempdir().unwrap();
    let history = PromptHistory::new(dir.path(), "/project", &test_session_id("session-1"));
    let payload = "界".repeat((MAX_INLINE_TEXT_BYTES / "界".len()) + 2);
    let label = "[Pasted text #1]";
    history
        .add_composer(&coco_types::PersistedComposer {
            text: label.into(),
            next_attachment_label: 1,
            elements: vec![coco_types::PersistedComposerElement::Paste {
                start: 0,
                end: i64::try_from(label.len()).unwrap(),
                content: payload.clone(),
            }],
        })
        .unwrap();

    let blobs = std::fs::read_dir(dir.path().join("composer-store"))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(blobs.len(), 1);
    assert_eq!(history.get_history()[0].composer.elements.len(), 1);
    assert!(matches!(
        history.get_history()[0].composer.elements.as_slice(),
        [coco_types::PersistedComposerElement::Paste { content, .. }]
            if content == &payload
    ));

    std::fs::write(blobs[0].path(), b"corrupt utf-8 paste").unwrap();
    assert!(history.get_history().is_empty());
}

#[test]
fn image_history_is_content_addressed_and_hash_verified() {
    use base64::Engine as _;

    let dir = tempfile::tempdir().unwrap();
    let history = PromptHistory::new(dir.path(), "/project", &test_session_id("session-1"));
    let bytes = b"durable image bytes";
    let label = "[Image #1]";
    history
        .add_composer(&coco_types::PersistedComposer {
            text: format!("inspect {label}"),
            next_attachment_label: 1,
            elements: vec![coco_types::PersistedComposerElement::Image {
                start: 8,
                end: i64::try_from(8 + label.len()).unwrap(),
                media_type: "image/png".into(),
                data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            }],
        })
        .unwrap();

    let store = dir.path().join("composer-store");
    let blobs = std::fs::read_dir(&store)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(blobs.len(), 1);
    assert_eq!(std::fs::read(blobs[0].path()).unwrap(), bytes);

    let entries = history.get_history();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].composer.next_attachment_label, 1);
    assert!(matches!(
        entries[0].composer.elements.as_slice(),
        [coco_types::PersistedComposerElement::Image { data_base64, .. }]
            if base64::engine::general_purpose::STANDARD.decode(data_base64).unwrap() == bytes
    ));

    std::fs::write(blobs[0].path(), b"corrupt").unwrap();
    assert!(history.get_history().is_empty());
}

#[test]
fn invalid_composer_ranges_are_rejected_before_writing_payloads() {
    use base64::Engine as _;

    let dir = tempfile::tempdir().unwrap();
    let history = PromptHistory::new(dir.path(), "/project", &test_session_id("session-1"));
    let result = history.add_composer(&coco_types::PersistedComposer {
        text: "é[Image #1]".into(),
        next_attachment_label: 1,
        elements: vec![coco_types::PersistedComposerElement::Image {
            start: 1,
            end: 12,
            media_type: "image/png".into(),
            data_base64: base64::engine::general_purpose::STANDARD.encode([1, 2, 3]),
        }],
    });

    assert!(result.is_err());
    assert!(!dir.path().join("composer-store").exists());
    assert!(!dir.path().join("history.jsonl").exists());
}

#[test]
fn attachment_store_removes_unreferenced_blobs_and_rejects_oversized_values() {
    let dir = tempfile::tempdir().unwrap();
    let history = PromptHistory::new(dir.path(), "/project", &test_session_id("session-1"));
    let store = dir.path().join("composer-store");
    std::fs::create_dir_all(&store).unwrap();
    let orphan_hash = hash_bytes(b"orphan");
    let orphan = store.join(&orphan_hash);
    std::fs::write(&orphan, b"orphan").unwrap();

    history.add("trigger collection").unwrap();
    assert!(!orphan.exists());

    let oversized = vec![0; MAX_ATTACHMENT_BLOB_BYTES + 1];
    let hash = hash_bytes(&oversized);
    assert!(history.write_blob(&hash, &oversized).is_err());
    assert!(!store.join(hash).exists());
}
