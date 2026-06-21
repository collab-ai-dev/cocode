use super::*;
use crate::generator::GeneratorContext;
use coco_config::SystemReminderConfig;
use pretty_assertions::assert_eq;

fn cfg_with_ultrathink_enabled() -> SystemReminderConfig {
    let mut c = SystemReminderConfig::default();
    c.attachments.ultrathink_effort = true;
    c
}

#[tokio::test]
async fn skips_when_config_disabled() {
    let c = SystemReminderConfig::default();
    assert!(!c.attachments.ultrathink_effort);
    assert!(!UltrathinkEffortGenerator.is_enabled(&c));
}

#[tokio::test]
async fn skips_when_user_input_absent() {
    let c = cfg_with_ultrathink_enabled();
    let ctx = GeneratorContext::builder(&c).user_input(None).build();
    assert!(
        UltrathinkEffortGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn skips_when_keyword_absent() {
    let c = cfg_with_ultrathink_enabled();
    let ctx = GeneratorContext::builder(&c)
        .user_input(Some("think harder please".to_string()))
        .build();
    assert!(
        UltrathinkEffortGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn emits_when_keyword_present() {
    let c = cfg_with_ultrathink_enabled();
    let ctx = GeneratorContext::builder(&c)
        .user_input(Some("ultrathink about this design".to_string()))
        .build();
    let r = UltrathinkEffortGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .expect("emits");
    assert_eq!(r.attachment_type, AttachmentType::UltrathinkEffort);
    let text = r.content().unwrap();
    assert!(text.contains("reasoning effort level: high"));
    assert!(text.contains("Apply this to the current turn"));
}

#[tokio::test]
async fn keyword_match_is_case_insensitive_with_word_boundaries() {
    // UltraThink / ULTRATHINK / in-sentence all match; substring doesn't.
    assert!(contains_ultrathink_keyword("UltraThink now"));
    assert!(contains_ultrathink_keyword("please ULTRATHINK"));
    assert!(contains_ultrathink_keyword("before: ultrathink, after"));
    // Word-boundary: 'ultrathinking' should not match (not a whole word).
    assert!(!contains_ultrathink_keyword("ultrathinking more"));
    // Unrelated text
    assert!(!contains_ultrathink_keyword("ultra thinking"));
    assert!(!contains_ultrathink_keyword(""));
}
