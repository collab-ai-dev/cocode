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

#[tokio::test]
async fn bounds_the_aggregate_listing() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .skill_listing(Some("- skill: verbose description\n".repeat(1_000)))
        .build();
    let text = SkillListingGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();

    assert!(text.len() <= coco_context::SkillListingFragment::MAX_BYTES);
    assert!(
        coco_messages::estimate_text_tokens(&text)
            <= coco_context::SkillListingFragment::MAX_TOKENS
    );

    let injected = crate::create_injected_messages(vec![SystemReminder::new(
        AttachmentType::SkillListing,
        text,
    )]);
    let [crate::InjectedMessage::UserText { content, .. }] = injected.as_slice() else {
        panic!("expected one injected skill-listing message");
    };
    assert!(content.len() <= coco_context::SkillListingFragment::MAX_RENDERED_BYTES);
    assert!(
        coco_messages::estimate_text_tokens(content)
            <= coco_context::SkillListingFragment::MAX_RENDERED_TOKENS
    );
}
