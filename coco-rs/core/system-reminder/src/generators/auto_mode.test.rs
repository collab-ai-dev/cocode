use super::*;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use coco_config::SystemReminderConfig;

fn cfg() -> SystemReminderConfig {
    SystemReminderConfig::default()
}

#[tokio::test]
async fn exit_emits_only_when_flag_set() {
    let c = cfg();
    let g = AutoModeExitGenerator;

    let off = GeneratorContext::builder(&c)
        .needs_auto_mode_exit_attachment(false)
        .build();
    assert!(g.generate(&off).await.unwrap().is_none());

    let on = GeneratorContext::builder(&c)
        .needs_auto_mode_exit_attachment(true)
        .build();
    let r = g.generate(&on).await.unwrap().expect("emits");
    assert_eq!(r.attachment_type, AttachmentType::AutoModeExit);
    let text = r.content().expect("text");
    assert!(text.contains("Exited Auto Mode"), "banner text: {text}");
}

#[tokio::test]
async fn exit_suppressed_when_still_in_auto_mode() {
    let c = cfg();
    // Flag set AND engine still in auto → suppress (stale flag).
    let ctx = GeneratorContext::builder(&c)
        .needs_auto_mode_exit_attachment(true)
        .is_auto_mode(true)
        .build();
    assert!(
        AutoModeExitGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn exit_respects_config_flag() {
    let mut c = cfg();
    c.attachments.auto_mode_exit = false;
    assert!(!AutoModeExitGenerator.is_enabled(&c));
}

#[tokio::test]
async fn exit_attachment_type_identity() {
    assert_eq!(
        AutoModeExitGenerator.attachment_type(),
        AttachmentType::AutoModeExit
    );
    assert_eq!(AutoModeExitGenerator.name(), "AutoModeExitGenerator");
}
