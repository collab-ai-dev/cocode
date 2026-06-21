use super::*;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use coco_config::SystemReminderConfig;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn none_when_new_date_unset() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c).new_date(None).build();
    assert!(DateChangeGenerator.generate(&ctx).await.unwrap().is_none());
}

#[tokio::test]
async fn none_when_new_date_empty() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .new_date(Some(String::new()))
        .build();
    assert!(DateChangeGenerator.generate(&ctx).await.unwrap().is_none());
}

#[tokio::test]
async fn emits_with_ts_body_when_date_present() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .new_date(Some("2026-04-21".to_string()))
        .build();
    let r = DateChangeGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .expect("emits");
    assert_eq!(r.attachment_type, AttachmentType::DateChange);
    assert_eq!(
        r.content(),
        Some(
            "The date has changed. Today's date is now 2026-04-21. DO NOT mention this to the user explicitly because they are already aware."
        )
    );
}

#[tokio::test]
async fn respects_config_flag() {
    let mut c = SystemReminderConfig::default();
    c.attachments.date_change = false;
    assert!(!DateChangeGenerator.is_enabled(&c));
}
