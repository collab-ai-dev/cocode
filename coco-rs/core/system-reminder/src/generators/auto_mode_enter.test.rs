use super::*;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use coco_config::SystemReminderConfig;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn none_when_not_auto_mode() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c).is_auto_mode(false).build();
    assert!(
        AutoModeEnterGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn emits_full_content_when_full_flag_set() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .is_auto_mode(true)
        .auto_mode_attachments_since_exit(0)
        .build();
    let r = AutoModeEnterGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .expect("emits");
    assert_eq!(r.attachment_type, AttachmentType::AutoMode);
    let text = r.content().unwrap();
    // Full text contains the 6-point list and the "## Auto Mode Active" header.
    assert!(text.starts_with("## Auto Mode Active"), "header: {text}");
    assert!(text.contains("Execute immediately"));
    assert!(text.contains("Avoid data exfiltration"));
}

#[tokio::test]
async fn emits_sparse_content_when_full_flag_unset() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .is_auto_mode(true)
        .auto_mode_attachments_since_exit(1)
        .build();
    let r = AutoModeEnterGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .expect("emits");
    let text = r.content().unwrap();
    assert!(text.contains("Auto mode still active"));
    assert!(
        !text.contains("## Auto Mode Active"),
        "sparse must not include full header: {text}"
    );
}

#[tokio::test]
async fn respects_config_flag() {
    let mut c = SystemReminderConfig::default();
    c.attachments.auto_mode = false;
    assert!(!AutoModeEnterGenerator.is_enabled(&c));
}

#[tokio::test]
async fn cadence_is_history_derived() {
    let c = SystemReminderConfig::default();
    let g = AutoModeEnterGenerator;

    // First auto-mode turn (no prior attachment) always emits.
    let first = GeneratorContext::builder(&c)
        .is_auto_mode(true)
        .auto_mode_turns_since_attachment(None)
        .build();
    assert!(g.generate(&first).await.unwrap().is_some());

    // Within the 5-turn window → throttled.
    let within = GeneratorContext::builder(&c)
        .is_auto_mode(true)
        .auto_mode_turns_since_attachment(Some(2))
        .build();
    assert!(g.generate(&within).await.unwrap().is_none());

    // At/after the window → emits again.
    let after = GeneratorContext::builder(&c)
        .is_auto_mode(true)
        .auto_mode_turns_since_attachment(Some(5))
        .build();
    assert!(g.generate(&after).await.unwrap().is_some());
}
