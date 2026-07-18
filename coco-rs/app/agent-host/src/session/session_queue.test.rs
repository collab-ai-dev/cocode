use super::*;

#[test]
fn human_queued_command_marks_origin_and_images() {
    let images = vec![QueuedImage {
        media_type: "image/png".to_string(),
        data_base64: "abc".to_string(),
        insertion_offset: 2,
    }];
    let command = human_queued_command("hello".to_string(), images.clone(), Default::default());
    assert_eq!(command.priority, QueuePriority::Next);
    assert_eq!(command.origin, Some(QueueOrigin::Human));
    assert_eq!(command.images, images);
    assert!(!command.is_slash_command);
}

#[test]
fn human_queued_command_detects_slash_commands() {
    let command = human_queued_command("  /status".to_string(), Vec::new(), Default::default());
    assert!(command.is_slash_command);
}

#[test]
fn coordinator_queued_command_tags_origin() {
    let framed = "<teammate_message teammate_id=\"alice\">done</teammate_message>".to_string();
    let command = coordinator_queued_command(framed.clone());
    assert_eq!(command.priority, QueuePriority::Later);
    assert_eq!(command.origin, Some(QueueOrigin::Coordinator));
    assert_eq!(command.prompt, framed);
    assert!(
        !command.is_slash_command,
        "teammate XML must not be parsed as a slash command"
    );
}

#[test]
fn cron_queued_command_tags_origin() {
    let command = cron_queued_command("run scheduled task".to_string());
    assert_eq!(command.priority, QueuePriority::Later);
    assert_eq!(command.origin, Some(QueueOrigin::Cron));
}

#[test]
fn queued_images_to_wire_preserves_image_payloads() {
    let images = queued_images_to_wire(vec![QueuedImage {
        media_type: "image/jpeg".to_string(),
        data_base64: "xyz".to_string(),
        insertion_offset: 4,
    }]);
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].media_type, "image/jpeg");
    assert_eq!(images[0].data_base64, "xyz");
    assert_eq!(images[0].insertion_offset, 4);
}

#[test]
fn queued_commands_for_edit_appends_current_input_and_projects_images() {
    let first = human_queued_command(
        "first".to_string(),
        vec![QueuedImage {
            media_type: "image/png".to_string(),
            data_base64: "png".to_string(),
            insertion_offset: 2,
        }],
        coco_types::SubmittedComposer {
            next_attachment_label: 1,
            elements: vec![
                coco_types::SubmittedComposerElement::FileRef { start: 0, end: 2 },
                coco_types::SubmittedComposerElement::Image {
                    insertion_offset: 2,
                    image_index: 0,
                    label: "[Image #1]".into(),
                },
            ],
        },
    );
    let first_id = first.id.to_string();
    let second = human_queued_command(
        "second".to_string(),
        vec![QueuedImage {
            media_type: "image/jpeg".to_string(),
            data_base64: "jpeg".to_string(),
            insertion_offset: 1,
        }],
        coco_types::SubmittedComposer {
            next_attachment_label: 2,
            elements: vec![coco_types::SubmittedComposerElement::Image {
                insertion_offset: 1,
                image_index: 0,
                label: "[Image #2]".into(),
            }],
        },
    );
    let second_id = second.id.to_string();

    let edit = queued_commands_for_edit(&[first, second], 3);

    assert_eq!(edit.ids, vec![first_id, second_id]);
    assert_eq!(edit.prompt, "first\nsecond");
    assert_eq!(edit.remaining_queued, 3);
    assert_eq!(edit.images.len(), 2);
    assert_eq!(edit.images[0].media_type, "image/png");
    assert_eq!(edit.images[0].data_base64, "png");
    assert_eq!(edit.images[0].insertion_offset, 2);
    assert_eq!(edit.images[1].media_type, "image/jpeg");
    assert_eq!(edit.images[1].data_base64, "jpeg");
    assert_eq!(edit.images[1].insertion_offset, 7);
    assert_eq!(edit.composer.next_attachment_label, 2);
    assert!(matches!(
        edit.composer.elements.as_slice(),
        [
            coco_types::SubmittedComposerElement::FileRef { start: 0, end: 2 },
            coco_types::SubmittedComposerElement::Image {
                insertion_offset: 2,
                image_index: 0,
                ..
            },
            coco_types::SubmittedComposerElement::Image {
                insertion_offset: 7,
                image_index: 1,
                ..
            }
        ]
    ));
}
