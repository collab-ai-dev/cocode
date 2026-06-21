use super::*;
use crate::generator::GeneratorContext;
use coco_config::SystemReminderConfig;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn default_config_is_enabled() {
    let c = SystemReminderConfig::default();
    assert!(BudgetUsdGenerator.is_enabled(&c));
}

#[tokio::test]
async fn skips_when_budget_unset() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .max_budget_usd(None)
        .total_cost_usd(1.23)
        .build();
    assert!(BudgetUsdGenerator.generate(&ctx).await.unwrap().is_none());
}

#[tokio::test]
async fn emits_with_ts_format() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .max_budget_usd(Some(10.0))
        .total_cost_usd(3.5)
        .build();
    let r = BudgetUsdGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .expect("emits");
    assert_eq!(r.attachment_type, AttachmentType::BudgetUsd);
    let text = r.content().unwrap();
    // Expected format: `USD budget: $3.5/$10; $6.5 remaining`
    assert!(text.starts_with("USD budget:"));
    assert!(text.contains("$3.5/$10"));
    assert!(text.contains("$6.5 remaining"));
}
