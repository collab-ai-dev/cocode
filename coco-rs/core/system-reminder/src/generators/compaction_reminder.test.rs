use super::*;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use coco_config::SystemReminderConfig;

fn ctx_with<'a>(
    c: &'a SystemReminderConfig,
    auto_compact: bool,
    window: i64,
    effective: i64,
    used: i64,
) -> GeneratorContext<'a> {
    GeneratorContext::builder(c)
        .is_auto_compact_enabled(auto_compact)
        .context_window(window)
        .effective_context_window(effective)
        .used_tokens(used)
        .build()
}

#[tokio::test]
async fn none_when_auto_compact_disabled() {
    let c = SystemReminderConfig::default();
    let ctx = ctx_with(&c, false, 1_000_000, 900_000, 500_000);
    assert!(
        CompactionReminderGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn none_when_context_below_1m() {
    let c = SystemReminderConfig::default();
    let ctx = ctx_with(&c, true, 200_000, 180_000, 100_000);
    assert!(
        CompactionReminderGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn none_when_usage_below_25_percent() {
    let c = SystemReminderConfig::default();
    // 100k / 1M effective = 10%, well below 25%.
    let ctx = ctx_with(&c, true, 1_000_000, 1_000_000, 100_000);
    assert!(
        CompactionReminderGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn none_when_effective_window_missing() {
    let c = SystemReminderConfig::default();
    let ctx = ctx_with(&c, true, 1_000_000, 0, 500_000);
    assert!(
        CompactionReminderGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn emits_at_exactly_25_percent() {
    let c = SystemReminderConfig::default();
    // 250k / 1M = 25% exactly — threshold uses `>=` so must fire.
    let ctx = ctx_with(&c, true, 1_000_000, 1_000_000, 250_000);
    let r = CompactionReminderGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .expect("emits");
    assert_eq!(r.attachment_type, AttachmentType::CompactionReminder);
}

#[tokio::test]
async fn emits_above_threshold_with_ts_body() {
    let c = SystemReminderConfig::default();
    let ctx = ctx_with(&c, true, 1_500_000, 1_400_000, 800_000);
    let r = CompactionReminderGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap();
    let text = r.content().unwrap();
    assert!(text.contains("Auto-compact is enabled."));
    assert!(text.contains("unlimited context through automatic compaction"));
}

#[tokio::test]
async fn respects_config_flag() {
    let mut c = SystemReminderConfig::default();
    c.attachments.compaction_reminder = false;
    assert!(!CompactionReminderGenerator.is_enabled(&c));
}
