use super::*;
use crate::generator::GeneratorContext;
use coco_config::SystemReminderConfig;
use pretty_assertions::assert_eq;

fn cfg_enabled() -> SystemReminderConfig {
    let mut c = SystemReminderConfig::default();
    c.attachments.companion_intro = true;
    c
}

#[tokio::test]
async fn respects_config_flag() {
    let c = SystemReminderConfig::default();
    assert!(!CompanionIntroGenerator.is_enabled(&c));
}

#[tokio::test]
async fn skips_when_companion_not_configured() {
    let c = cfg_enabled();
    let ctx = GeneratorContext::builder(&c)
        .companion(None, None)
        .has_prior_companion_intro(false)
        .build();
    assert!(
        CompanionIntroGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn skips_when_already_announced() {
    let c = cfg_enabled();
    let ctx = GeneratorContext::builder(&c)
        .companion(Some("Pebble".to_string()), Some("rabbit".to_string()))
        .has_prior_companion_intro(true)
        .build();
    assert!(
        CompanionIntroGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn emits_with_ts_text_template() {
    let c = cfg_enabled();
    let ctx = GeneratorContext::builder(&c)
        .companion(Some("Pebble".to_string()), Some("rabbit".to_string()))
        .has_prior_companion_intro(false)
        .build();
    let r = CompanionIntroGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .expect("emits");
    assert_eq!(r.attachment_type, AttachmentType::CompanionIntro);
    let text = r.content().unwrap();
    assert!(text.starts_with("# Companion"));
    assert!(text.contains("A small rabbit named Pebble"));
    assert!(text.contains("You're not Pebble"));
    assert!(text.contains("ONE line or less"));
}
