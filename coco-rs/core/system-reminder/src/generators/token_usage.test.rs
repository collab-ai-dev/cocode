use super::*;
use crate::generator::GeneratorContext;
use coco_config::SystemReminderConfig;
use pretty_assertions::assert_eq;

fn cfg_enabled() -> SystemReminderConfig {
    let mut c = SystemReminderConfig::default();
    c.attachments.token_usage = true;
    c
}

#[tokio::test]
async fn respects_config_flag() {
    let c = SystemReminderConfig::default();
    assert!(!TokenUsageGenerator.is_enabled(&c));
}

#[tokio::test]
async fn skips_when_window_zero() {
    let c = cfg_enabled();
    let ctx = GeneratorContext::builder(&c)
        .effective_context_window(0)
        .used_tokens(1000)
        .build();
    assert!(TokenUsageGenerator.generate(&ctx).await.unwrap().is_none());
}

#[tokio::test]
async fn emits_with_ts_format() {
    let c = cfg_enabled();
    let ctx = GeneratorContext::builder(&c)
        .effective_context_window(200_000)
        .used_tokens(25_000)
        .build();
    let r = TokenUsageGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .expect("emits");
    assert_eq!(r.attachment_type, AttachmentType::TokenUsage);
    let text = r.content().unwrap();
    assert_eq!(text, "Token usage: 25000/200000; 175000 remaining");
}

#[tokio::test]
async fn clamps_negative_remaining_to_zero() {
    let c = cfg_enabled();
    let ctx = GeneratorContext::builder(&c)
        .effective_context_window(1000)
        .used_tokens(1500) // over budget — guard against negative remaining
        .build();
    let r = TokenUsageGenerator.generate(&ctx).await.unwrap().unwrap();
    let text = r.content().unwrap();
    assert!(text.contains("0 remaining"), "{text}");
}
