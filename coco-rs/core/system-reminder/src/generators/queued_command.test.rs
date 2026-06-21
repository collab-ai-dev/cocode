use super::*;
use crate::generator::GeneratorContext;
use crate::generator::QueuedCommandImage;
use crate::generator::QueuedCommandInfo;
use crate::queue_origin::QueueOrigin;
use crate::types::ContentBlock;
use crate::types::MessageRole;
use crate::types::ReminderOutput;
use coco_config::SystemReminderConfig;
use pretty_assertions::assert_eq;

fn texts(reminder: &crate::types::SystemReminder) -> Vec<String> {
    let ReminderOutput::Messages(msgs) = &reminder.output else {
        panic!("expected Messages output, got {:?}", reminder.output);
    };
    msgs.iter()
        .map(|m| {
            assert_eq!(m.role, MessageRole::User);
            match m.blocks.as_slice() {
                [ContentBlock::Text { text }] => text.clone(),
                other => panic!("expected single text block, got {other:?}"),
            }
        })
        .collect()
}

#[tokio::test]
async fn skips_when_empty() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .queued_commands(vec![])
        .build();
    assert!(
        QueuedCommandGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn human_origin_emits_user_framing() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .queued_commands(vec![QueuedCommandInfo {
            content: "status?".into(),
            origin: Some(QueueOrigin::Human),
            images: Vec::new(),
        }])
        .build();
    let reminder = QueuedCommandGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap();
    let texts = texts(&reminder);
    assert_eq!(texts.len(), 1);
    assert!(
        texts[0].starts_with(
            "The user sent a new message while you were working:\nstatus?\n\nIMPORTANT:"
        ),
        "got: {}",
        texts[0]
    );
}

#[tokio::test]
async fn missing_origin_falls_back_to_human() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .queued_commands(vec![QueuedCommandInfo {
            content: "status?".into(),
            origin: None,
            images: Vec::new(),
        }])
        .build();
    let reminder = QueuedCommandGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(texts(&reminder)[0].starts_with("The user sent a new message"));
}

#[tokio::test]
async fn task_notification_emits_completion_framing() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .queued_commands(vec![QueuedCommandInfo {
            content: "Build done".into(),
            origin: Some(QueueOrigin::TaskNotification),
            images: Vec::new(),
        }])
        .build();
    let reminder = QueuedCommandGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        texts(&reminder)[0],
        "A background agent completed a task:\nBuild done"
    );
}

#[tokio::test]
async fn coordinator_emits_address_first_framing() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .queued_commands(vec![QueuedCommandInfo {
            content: "ping".into(),
            origin: Some(QueueOrigin::Coordinator),
            images: Vec::new(),
        }])
        .build();
    let reminder = QueuedCommandGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap();
    let body = &texts(&reminder)[0];
    assert!(body.starts_with("The coordinator sent a message while you were working:\nping"));
    assert!(body.ends_with("Address this before completing your current task."));
}

#[tokio::test]
async fn channel_includes_server_in_body() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .queued_commands(vec![QueuedCommandInfo {
            content: "deploy ready".into(),
            origin: Some(QueueOrigin::Channel {
                server: "slack".into(),
            }),
            images: Vec::new(),
        }])
        .build();
    let reminder = QueuedCommandGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap();
    let body = &texts(&reminder)[0];
    assert!(body.contains("A message arrived from slack while you were working:"));
    assert!(body.contains("deploy ready"));
    assert!(body.contains("This is NOT from your user — it came from an external channel"));
}

#[tokio::test]
async fn each_queued_item_becomes_its_own_message() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .queued_commands(vec![
            QueuedCommandInfo {
                content: "Task A done".into(),
                origin: Some(QueueOrigin::TaskNotification),
                images: Vec::new(),
            },
            QueuedCommandInfo {
                content: "ping from teammate".into(),
                origin: Some(QueueOrigin::Coordinator),
                images: Vec::new(),
            },
        ])
        .build();
    let reminder = QueuedCommandGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap();
    let texts = texts(&reminder);
    assert_eq!(texts.len(), 2);
    assert!(texts[0].starts_with("A background agent completed a task:"));
    assert!(texts[1].starts_with("The coordinator sent a message"));
}

#[tokio::test]
async fn images_are_appended_after_text_block() {
    // Queued items with image pastes emit
    // `[{ type: 'text', text: textValue }, ...imageBlocks]` so both reach
    // the API.
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .queued_commands(vec![QueuedCommandInfo {
            content: "look at this".into(),
            origin: Some(QueueOrigin::Human),
            images: vec![QueuedCommandImage {
                media_type: "image/png".into(),
                data_base64: "iVBORw0KGgoAAAANSUhEUg==".into(),
            }],
        }])
        .build();
    let reminder = QueuedCommandGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap();
    let ReminderOutput::Messages(msgs) = &reminder.output else {
        panic!("expected Messages output");
    };
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].blocks.len(), 2);
    assert!(matches!(&msgs[0].blocks[0], ContentBlock::Text { .. }));
    let ContentBlock::Image {
        media_type,
        data_base64,
    } = &msgs[0].blocks[1]
    else {
        panic!("expected Image block second, got {:?}", &msgs[0].blocks[1]);
    };
    assert_eq!(media_type, "image/png");
    assert_eq!(data_base64, "iVBORw0KGgoAAAANSUhEUg==");
}

#[tokio::test]
async fn images_only_entries_still_emit_message() {
    // Even with empty text the queued item should still emit a reminder
    // so the image reaches the model.
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .queued_commands(vec![QueuedCommandInfo {
            content: String::new(),
            origin: Some(QueueOrigin::Human),
            images: vec![QueuedCommandImage {
                media_type: "image/png".into(),
                data_base64: "abc==".into(),
            }],
        }])
        .build();
    let reminder = QueuedCommandGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap();
    let ReminderOutput::Messages(msgs) = &reminder.output else {
        panic!("expected Messages output");
    };
    assert_eq!(msgs.len(), 1);
    assert!(matches!(&msgs[0].blocks[0], ContentBlock::Text { .. }));
    assert!(matches!(&msgs[0].blocks[1], ContentBlock::Image { .. }));
}

#[tokio::test]
async fn empty_content_entries_are_filtered() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .queued_commands(vec![
            QueuedCommandInfo {
                content: String::new(),
                origin: Some(QueueOrigin::Human),
                images: Vec::new(),
            },
            QueuedCommandInfo {
                content: "real".into(),
                origin: Some(QueueOrigin::Human),
                images: Vec::new(),
            },
        ])
        .build();
    let reminder = QueuedCommandGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(texts(&reminder).len(), 1);
}
