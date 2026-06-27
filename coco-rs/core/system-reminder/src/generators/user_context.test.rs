use super::*;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use coco_config::SystemReminderConfig;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn none_when_current_date_unset() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c).current_date(None).build();
    assert!(UserContextGenerator.generate(&ctx).await.unwrap().is_none());
}

#[tokio::test]
async fn none_when_current_date_empty() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .current_date(Some(String::new()))
        .build();
    assert!(UserContextGenerator.generate(&ctx).await.unwrap().is_none());
}

#[tokio::test]
async fn emits_prepend_user_context_body_when_date_present() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .current_date(Some("2026-06-05".to_string()))
        .build();
    let r = UserContextGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .expect("emits");
    assert_eq!(r.attachment_type, AttachmentType::UserContext);
    // currentDate-only context map. Six-space indent before IMPORTANT is
    // a template-literal artifact preserved for model compatibility.
    assert_eq!(
        r.content(),
        Some(
            "As you answer the user's questions, you can use the following context:\n# currentDate\nToday's date is 2026-06-05.\n\n      IMPORTANT: this context may or may not be relevant to your tasks. You should not respond to this context unless it is highly relevant to your task."
        )
    );
}

#[tokio::test]
async fn emits_worker_context_block_when_only_coordinator_context_present() {
    // Coordinator (leader) path with no date: the body must still emit,
    // carrying just the `# workerToolsContext` block.
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .current_date(None)
        .coordinator_worker_context(Some(
            "Workers spawned via the Agent tool have access to these tools: Bash, Read".to_string(),
        ))
        .build();
    let r = UserContextGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .expect("emits");
    assert_eq!(
        r.content(),
        Some(
            "As you answer the user's questions, you can use the following context:\n# workerToolsContext\nWorkers spawned via the Agent tool have access to these tools: Bash, Read\n\n      IMPORTANT: this context may or may not be relevant to your tasks. You should not respond to this context unless it is highly relevant to your task."
        )
    );
}

#[tokio::test]
async fn emits_both_blocks_date_then_worker_context() {
    // Both keys present: currentDate first, workerToolsContext second,
    // each as its own `# key\nvalue` block.
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .current_date(Some("2026-06-13".to_string()))
        .coordinator_worker_context(Some("Workers have access to: Bash, Read, Edit".to_string()))
        .build();
    let r = UserContextGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .expect("emits");
    assert_eq!(
        r.content(),
        Some(
            "As you answer the user's questions, you can use the following context:\n# currentDate\nToday's date is 2026-06-13.\n# workerToolsContext\nWorkers have access to: Bash, Read, Edit\n\n      IMPORTANT: this context may or may not be relevant to your tasks. You should not respond to this context unless it is highly relevant to your task."
        )
    );
}

#[tokio::test]
async fn none_when_both_date_and_worker_context_unset() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .current_date(None)
        .coordinator_worker_context(None)
        .build();
    assert!(UserContextGenerator.generate(&ctx).await.unwrap().is_none());
}

#[tokio::test]
async fn fires_every_turn_via_core_tier() {
    assert_eq!(
        UserContextGenerator.attachment_type().tier(),
        crate::types::ReminderTier::Core
    );
}

#[tokio::test]
async fn respects_config_flag() {
    let mut c = SystemReminderConfig::default();
    c.attachments.user_context = false;
    assert!(!UserContextGenerator.is_enabled(&c));
}
