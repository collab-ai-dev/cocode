use super::*;
use crate::generator::GeneratorContext;
use crate::generator::OutputStyleSnapshot;
use coco_config::SystemReminderConfig;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn skips_when_none() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c).output_style(None).build();
    assert!(OutputStyleGenerator.generate(&ctx).await.unwrap().is_none());
}

#[tokio::test]
async fn skips_when_name_empty() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .output_style(Some(OutputStyleSnapshot {
            name: String::new(),
        }))
        .build();
    assert!(OutputStyleGenerator.generate(&ctx).await.unwrap().is_none());
}

#[tokio::test]
async fn emits_with_ts_template() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .output_style(Some(OutputStyleSnapshot {
            name: "Explanatory".into(),
        }))
        .build();
    let text = OutputStyleGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    assert_eq!(
        text,
        "Explanatory output style is active. Remember to follow the specific guidelines for this style."
    );
}
