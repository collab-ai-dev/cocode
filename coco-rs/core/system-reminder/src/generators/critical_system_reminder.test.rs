use super::*;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use coco_config::SystemReminderConfig;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn none_when_instruction_unset() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c).build();
    assert!(
        CriticalSystemReminderGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn none_when_instruction_blank() {
    let c = SystemReminderConfig {
        critical_instruction: Some("   \n\t".to_string()),
        ..Default::default()
    };
    let ctx = GeneratorContext::builder(&c).build();
    assert!(
        CriticalSystemReminderGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn emits_instruction_verbatim() {
    let c = SystemReminderConfig {
        critical_instruction: Some("Never touch the database without permission.".to_string()),
        ..Default::default()
    };
    let ctx = GeneratorContext::builder(&c).build();
    let r = CriticalSystemReminderGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .expect("emits");
    assert_eq!(r.attachment_type, AttachmentType::CriticalSystemReminder);
    assert_eq!(
        r.content(),
        Some("Never touch the database without permission.")
    );
}

#[tokio::test]
async fn emits_every_turn_without_throttle_state() {
    let c = SystemReminderConfig {
        critical_instruction: Some("be careful".to_string()),
        ..Default::default()
    };
    let ctx = GeneratorContext::builder(&c).turn_number(0).build();
    assert!(
        CriticalSystemReminderGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_some()
    );
    let ctx2 = GeneratorContext::builder(&c).turn_number(1).build();
    assert!(
        CriticalSystemReminderGenerator
            .generate(&ctx2)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn respects_config_flag() {
    let mut c = SystemReminderConfig {
        critical_instruction: Some("x".to_string()),
        ..Default::default()
    };
    c.attachments.critical_system_reminder = false;
    assert!(!CriticalSystemReminderGenerator.is_enabled(&c));
}
