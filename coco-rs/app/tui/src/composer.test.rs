use super::*;

#[test]
fn lookalike_labels_are_plain_text() {
    let mut textarea = TextArea::new();
    textarea.set_text("[Image #1]");
    let resolved = AttachmentStore::default().resolve(&textarea).unwrap();
    assert_eq!(resolved.text, "[Image #1]");
    assert!(resolved.images.is_empty());
}

#[test]
fn queued_images_restore_at_exact_offsets() {
    let images = vec![coco_types::QueuedCommandEditImage {
        media_type: "image/png".into(),
        data_base64: base64::engine::general_purpose::STANDARD.encode([1, 2, 3]),
        insertion_offset: 1,
    }];
    let snapshot =
        ComposerSnapshot::from_queued_edit("ab".into(), images, 0).expect("valid queued composer");
    assert_eq!(snapshot.text(), "a[Image #1]b");

    let mut textarea = TextArea::new();
    textarea.restore_snapshot(snapshot.textarea.clone());
    let resolved = snapshot.attachments.resolve(&textarea).unwrap();
    assert_eq!(resolved.text, "ab");
    assert_eq!(resolved.images[0].insertion_offset, 1);
    assert_eq!(resolved.images[0].bytes.as_ref(), [1, 2, 3]);
}

#[test]
fn mixed_unicode_composer_resolves_all_typed_offsets() {
    let mut textarea = TextArea::new();
    let mut attachments = AttachmentStore::default();
    textarea.insert_str("你");
    attachments.insert_text(&mut textarea, "αβ".into()).unwrap();
    textarea.insert_str("-");
    attachments
        .insert_image(&mut textarea, vec![9], "image/png".into())
        .unwrap();
    textarea.insert_str("-");
    textarea
        .insert_element("@文.rs", ElementKind::FileRef, file_ref_display("@文.rs"))
        .unwrap();

    let resolved = attachments.resolve(&textarea).unwrap();
    assert_eq!(resolved.text, "你αβ--@文.rs");
    assert_eq!(resolved.images.len(), 1);
    assert_eq!(resolved.images[0].insertion_offset, 8);
    assert!(matches!(
        resolved.submitted.elements.as_slice(),
        [
            coco_types::SubmittedComposerElement::Paste { .. },
            coco_types::SubmittedComposerElement::Image { .. },
            coco_types::SubmittedComposerElement::FileRef { start: 9, end: 16 }
        ]
    ));

    let persisted = attachments.persisted(&textarea).unwrap();
    let restored = ComposerSnapshot::from_persisted(persisted.clone()).unwrap();
    assert_eq!(snapshot_to_persisted(&restored).unwrap(), persisted);
}

#[test]
fn submitted_composer_roundtrips_paste_image_and_file_ref_losslessly() {
    let mut textarea = TextArea::new();
    let mut attachments = AttachmentStore::default();
    textarea.insert_str("你 ");
    attachments
        .insert_text(&mut textarea, "expanded paste".into())
        .unwrap();
    textarea.insert_str(" ");
    attachments
        .insert_image(&mut textarea, vec![5, 6], "image/png".into())
        .unwrap();
    textarea.insert_str(" ");
    textarea
        .insert_element(
            "@src/lib.rs",
            ElementKind::FileRef,
            file_ref_display("@src/lib.rs"),
        )
        .unwrap();
    let original = ComposerSnapshot::new(textarea.take_snapshot(), attachments);
    let mut textarea = TextArea::new();
    textarea.restore_snapshot(original.textarea.clone());
    let resolved_input = original.attachments.resolve(&textarea).unwrap();
    let images = resolved_input
        .images
        .iter()
        .map(|image| coco_types::QueuedCommandEditImage {
            media_type: image.mime.clone(),
            data_base64: base64::engine::general_purpose::STANDARD.encode(image.bytes.as_ref()),
            insertion_offset: i64::try_from(image.insertion_offset).unwrap(),
        })
        .collect();

    let rebuilt =
        ComposerSnapshot::from_submitted(resolved_input.text, images, resolved_input.submitted)
            .unwrap();

    assert_eq!(
        snapshot_to_persisted(&rebuilt).unwrap(),
        snapshot_to_persisted(&original).unwrap()
    );
}

#[test]
fn snapshot_merge_preserves_both_image_payloads() {
    fn queued(text: &str, byte: u8) -> ComposerSnapshot {
        ComposerSnapshot::from_queued_edit(
            text.into(),
            vec![coco_types::QueuedCommandEditImage {
                media_type: "image/png".into(),
                data_base64: base64::engine::general_purpose::STANDARD.encode([byte]),
                insertion_offset: i64::try_from(text.len()).unwrap(),
            }],
            0,
        )
        .unwrap()
    }

    let merged = queued("first", 1).merged_with(queued("second", 2)).unwrap();
    let mut textarea = TextArea::new();
    textarea.restore_snapshot(merged.textarea.clone());
    let resolved = merged.attachments.resolve(&textarea).unwrap();
    assert_eq!(resolved.text, "first\nsecond");
    assert_eq!(resolved.images.len(), 2);
    assert_eq!(resolved.images[0].bytes.as_ref(), [1]);
    assert_eq!(resolved.images[1].bytes.as_ref(), [2]);
    assert_eq!(merged.text(), "first[Image #1]\nsecond[Image #2]");
}

#[test]
fn snapshot_merge_relabel_growth_shifts_following_file_reference() {
    let image = base64::engine::general_purpose::STANDARD.encode([1]);
    let prefix = ComposerSnapshot::from_persisted(coco_types::PersistedComposer {
        text: "[Image #9]".into(),
        next_attachment_label: 9,
        elements: vec![coco_types::PersistedComposerElement::Image {
            start: 0,
            end: 10,
            media_type: "image/png".into(),
            data_base64: image.clone(),
        }],
    })
    .unwrap();
    let suffix = ComposerSnapshot::from_persisted(coco_types::PersistedComposer {
        text: "[Image #9] @x".into(),
        next_attachment_label: 9,
        elements: vec![
            coco_types::PersistedComposerElement::Image {
                start: 0,
                end: 10,
                media_type: "image/png".into(),
                data_base64: image,
            },
            coco_types::PersistedComposerElement::FileRef { start: 11, end: 13 },
        ],
    })
    .unwrap();

    let merged = prefix.merged_with(suffix).unwrap();
    let persisted = snapshot_to_persisted(&merged).unwrap();
    assert_eq!(persisted.text, "[Image #9]\n[Image #10] @x");
    assert!(matches!(
        persisted.elements.as_slice(),
        [
            coco_types::PersistedComposerElement::Image {
                start: 0,
                end: 10,
                ..
            },
            coco_types::PersistedComposerElement::Image {
                start: 11,
                end: 22,
                ..
            },
            coco_types::PersistedComposerElement::FileRef { start: 23, end: 25 }
        ]
    ));
}

#[test]
fn persisted_composer_preserves_monotonic_attachment_labels() {
    let mut textarea = TextArea::new();
    let mut attachments = AttachmentStore::default();
    attachments
        .insert_image(&mut textarea, vec![1], "image/png".into())
        .unwrap();
    let first = textarea.elements()[0].range().clone();
    textarea.replace_range(first, "");
    attachments.prune(&textarea);
    assert_eq!(
        attachments
            .insert_image(&mut textarea, vec![2], "image/png".into())
            .unwrap(),
        "[Image #2]"
    );

    let persisted = attachments.persisted(&textarea).unwrap();
    assert_eq!(persisted.next_attachment_label, 2);
    let restored = ComposerSnapshot::from_persisted(persisted).unwrap();
    let (snapshot, mut attachments) = restored.into_parts();
    let mut textarea = TextArea::new();
    textarea.restore_snapshot(snapshot);
    assert_eq!(
        attachments
            .insert_image(&mut textarea, vec![3], "image/png".into())
            .unwrap(),
        "[Image #3]"
    );
}

#[test]
fn persisted_composer_rejects_inconsistent_attachment_labels() {
    let result = ComposerSnapshot::from_persisted(coco_types::PersistedComposer {
        text: "[Image #2]".into(),
        next_attachment_label: 1,
        elements: vec![coco_types::PersistedComposerElement::Image {
            start: 0,
            end: 10,
            media_type: "image/png".into(),
            data_base64: base64::engine::general_purpose::STANDARD.encode([1]),
        }],
    });
    assert!(matches!(
        result,
        Err(ComposerBuildError::InvalidAttachmentLabel)
    ));
}

#[test]
fn user_message_submitted_composer_metadata_adjusts_to_restored_text() {
    let mut text = coco_messages::TextContent::new("  see @src/lib.rs  ");
    let submitted = coco_types::SubmittedComposer {
        next_attachment_label: 0,
        elements: vec![coco_types::SubmittedComposerElement::FileRef { start: 6, end: 17 }],
    };
    let mut metadata = text.provider_metadata.take().unwrap_or_default();
    metadata.set(
        "coco_submitted_composer",
        serde_json::to_value(&submitted).unwrap(),
    );
    text.provider_metadata = Some(metadata);
    let message =
        coco_messages::create_user_message_with_parts(vec![coco_messages::UserContent::Text(text)]);
    let coco_messages::Message::User(user) = message else {
        panic!("expected user message");
    };

    assert_eq!(submitted_composer_from_user_message(&user), Some(submitted));
    assert_eq!(
        submitted_composer_for_restored_text(&user, "see @src/lib.rs"),
        Some(coco_types::SubmittedComposer {
            next_attachment_label: 0,
            elements: vec![coco_types::SubmittedComposerElement::FileRef { start: 4, end: 15 }],
        })
    );
}

#[test]
fn malformed_user_message_submitted_composer_metadata_is_discarded() {
    let mut text = coco_messages::TextContent::new("é@src/lib.rs");
    let mut metadata = text.provider_metadata.take().unwrap_or_default();
    metadata.set(
        "coco_submitted_composer",
        serde_json::json!({ "elements": "not an array" }),
    );
    text.provider_metadata = Some(metadata);
    let message =
        coco_messages::create_user_message_with_parts(vec![coco_messages::UserContent::Text(text)]);
    let coco_messages::Message::User(user) = message else {
        panic!("expected user message");
    };

    assert!(submitted_composer_from_user_message(&user).is_none());
}

#[test]
fn external_editor_moves_images_by_opaque_markers() {
    let original = ComposerSnapshot::from_queued_edit(
        "before after".into(),
        vec![coco_types::QueuedCommandEditImage {
            media_type: "image/png".into(),
            data_base64: base64::engine::general_purpose::STANDARD.encode([7, 8]),
            insertion_offset: 7,
        }],
        0,
    )
    .unwrap();
    let (session, content) = ExternalEditorSession::prepare(original.clone()).unwrap();
    assert_eq!(session.finish(content.clone(), false).unwrap(), original);

    let marker = &session.elements[0].marker;
    let edited = content.replace(marker, "");
    let moved = format!("{marker}{edited}");
    let restored = session.finish(moved, true).unwrap();
    let mut textarea = TextArea::new();
    textarea.restore_snapshot(restored.textarea.clone());
    let resolved = restored.attachments.resolve(&textarea).unwrap();
    assert_eq!(resolved.text, "before after");
    assert_eq!(resolved.images[0].insertion_offset, 0);
    assert_eq!(resolved.images[0].bytes.as_ref(), [7, 8]);
    assert_eq!(restored.text(), "[Image #1]before after");
    assert_eq!(restored.next_attachment_label(), 1);
}

#[test]
fn external_editor_rejects_duplicate_element_markers() {
    let original = ComposerSnapshot::from_queued_edit(
        "text".into(),
        vec![coco_types::QueuedCommandEditImage {
            media_type: "image/png".into(),
            data_base64: base64::engine::general_purpose::STANDARD.encode([1]),
            insertion_offset: 2,
        }],
        0,
    )
    .unwrap();
    let (session, content) = ExternalEditorSession::prepare(original).unwrap();
    let duplicate = format!("{content}{}", session.elements[0].marker);
    assert!(matches!(
        session.finish(duplicate, true),
        Err(ExternalEditorError::DuplicateElementMarker)
    ));
}

#[test]
fn external_editor_preserves_unmodified_pastes_and_file_refs() {
    let mut textarea = TextArea::new();
    let mut attachments = AttachmentStore::default();
    textarea.insert_str("review ");
    attachments
        .insert_text(&mut textarea, "expanded payload".into())
        .unwrap();
    textarea.insert_str(" ");
    textarea
        .insert_element(
            "@src/lib.rs",
            ElementKind::FileRef,
            file_ref_display("@src/lib.rs"),
        )
        .unwrap();
    let original = ComposerSnapshot::new(textarea.take_snapshot(), attachments);
    let (session, mut content) = ExternalEditorSession::prepare(original).unwrap();
    content.push('!');

    let restored = session.finish(content, true).unwrap();
    let persisted = snapshot_to_persisted(&restored).unwrap();
    assert_eq!(restored.text(), "review [Pasted text #1] @src/lib.rs!");
    assert!(matches!(
        persisted.elements.as_slice(),
        [
            coco_types::PersistedComposerElement::Paste { content, .. },
            coco_types::PersistedComposerElement::FileRef { .. }
        ] if content == "expanded payload"
    ));
}

#[test]
fn external_editor_tampering_demotes_an_atomic_paste_to_plain_text() {
    let mut textarea = TextArea::new();
    let mut attachments = AttachmentStore::default();
    attachments
        .insert_text(&mut textarea, "secret payload".into())
        .unwrap();
    let original = ComposerSnapshot::new(textarea.take_snapshot(), attachments);
    let (session, content) = ExternalEditorSession::prepare(original).unwrap();
    let edited = content.replace("secret payload", "changed representation");

    let restored = session.finish(edited, true).unwrap();
    let persisted = snapshot_to_persisted(&restored).unwrap();
    assert_eq!(persisted.text, "changed representation");
    assert!(persisted.elements.is_empty());
}
