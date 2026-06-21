use super::*;
use crate::generator::GeneratorContext;
use coco_config::SystemReminderConfig;

#[tokio::test]
async fn skips_when_none() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c).skill_listing(None).build();
    assert!(
        SkillListingGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn skips_when_empty_string() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .skill_listing(Some(String::new()))
        .build();
    assert!(
        SkillListingGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn emits_prefixed_content() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .skill_listing(Some("- example: A sample skill".into()))
        .build();
    let text = SkillListingGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    assert!(text.starts_with("The following skills are available for use with the Skill tool:"));
    assert!(text.contains("- example: A sample skill"));
}
