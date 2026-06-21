use super::*;
use crate::generator::GeneratorContext;
use coco_config::SystemReminderConfig;
use pretty_assertions::assert_eq;

fn cfg_enabled() -> SystemReminderConfig {
    let mut c = SystemReminderConfig::default();
    c.attachments.output_token_usage = true;
    c
}

#[tokio::test]
async fn respects_config_flag() {
    let c = SystemReminderConfig::default();
    assert!(!OutputTokenUsageGenerator.is_enabled(&c));
}

#[tokio::test]
async fn skips_when_budget_none() {
    let c = cfg_enabled();
    let ctx = GeneratorContext::builder(&c)
        .output_token_budget(None)
        .output_tokens_turn(1000)
        .output_tokens_session(5000)
        .build();
    assert!(
        OutputTokenUsageGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn skips_when_budget_zero() {
    let c = cfg_enabled();
    let ctx = GeneratorContext::builder(&c)
        .output_token_budget(Some(0))
        .output_tokens_turn(1000)
        .output_tokens_session(5000)
        .build();
    assert!(
        OutputTokenUsageGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn emits_with_ts_format_and_thousands_separator() {
    let c = cfg_enabled();
    let ctx = GeneratorContext::builder(&c)
        .output_token_budget(Some(8_000))
        .output_tokens_turn(2_500)
        .output_tokens_session(125_000)
        .build();
    let r = OutputTokenUsageGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .expect("emits");
    assert_eq!(r.attachment_type, AttachmentType::OutputTokenUsage);
    let text = r.content().unwrap();
    // Expected format: `Output tokens — turn: 2,500 / 8,000 · session: 125,000`
    assert_eq!(
        text,
        "Output tokens \u{2014} turn: 2,500 / 8,000 \u{00b7} session: 125,000"
    );
}

#[test]
fn format_number_groups_by_three() {
    assert_eq!(format_number(0), "0");
    assert_eq!(format_number(7), "7");
    assert_eq!(format_number(1_000), "1,000");
    assert_eq!(format_number(12_345), "12,345");
    assert_eq!(format_number(1_234_567), "1,234,567");
    assert_eq!(format_number(-1_234_567), "-1,234,567");
}
