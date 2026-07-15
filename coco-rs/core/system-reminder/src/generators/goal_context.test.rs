use super::*;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use coco_config::SystemReminderConfig;

#[tokio::test]
async fn none_when_goal_context_unset() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c).goal_context(None).build();
    assert!(GoalContextGenerator.generate(&ctx).await.unwrap().is_none());
}

#[tokio::test]
async fn none_when_goal_context_empty() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .goal_context(Some(String::new()))
        .build();
    assert!(GoalContextGenerator.generate(&ctx).await.unwrap().is_none());
}

#[tokio::test]
async fn emits_goal_context_body_when_present() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .goal_context(Some("keep working; call report_goal_turn".to_string()))
        .build();
    let reminder = GoalContextGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .expect("emits");
    assert_eq!(reminder.attachment_type, AttachmentType::GoalContext);
    assert_eq!(
        reminder.content(),
        Some("keep working; call report_goal_turn")
    );
}
