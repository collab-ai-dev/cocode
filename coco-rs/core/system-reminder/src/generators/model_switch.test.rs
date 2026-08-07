use super::*;
use crate::generator::GeneratorContextBuilder;
use coco_config::SystemReminderConfig;
use pretty_assertions::assert_eq;

fn info() -> ModelSwitchInfo {
    ModelSwitchInfo {
        previous: "claude-sonnet-5".to_string(),
        current: "claude-opus-5".to_string(),
    }
}

#[tokio::test]
async fn test_generate_no_switch_emits_nothing() {
    let config = SystemReminderConfig::default();
    let ctx = GeneratorContextBuilder::new(&config).build();

    assert!(
        ModelSwitchGenerator
            .generate(&ctx)
            .await
            .expect("generator succeeds")
            .is_none()
    );
}

#[tokio::test]
async fn test_generate_switch_emits_both_ids() {
    let config = SystemReminderConfig::default();
    let ctx = GeneratorContextBuilder::new(&config)
        .model_switch(Some(info()))
        .build();

    let reminder = ModelSwitchGenerator
        .generate(&ctx)
        .await
        .expect("generator succeeds")
        .expect("switch emits a reminder");

    assert_eq!(reminder.attachment_type, AttachmentType::ModelSwitch);
    let text = reminder.output.as_text().unwrap_or_default();
    assert!(text.contains("claude-opus-5"), "names the current model");
    assert!(text.contains("claude-sonnet-5"), "names the previous model");
}

#[tokio::test]
async fn test_generate_retracts_the_stale_system_prompt_claim() {
    // The whole point: the prompt's `<env>` block still names the old model.
    // A reminder that only announces the new one leaves two contradictory
    // claims standing, so it has to say the earlier one is void.
    let config = SystemReminderConfig::default();
    let ctx = GeneratorContextBuilder::new(&config)
        .model_switch(Some(info()))
        .build();

    let reminder = ModelSwitchGenerator
        .generate(&ctx)
        .await
        .expect("generator succeeds")
        .expect("switch emits a reminder");
    let text = reminder.output.as_text().unwrap_or_default();

    assert!(text.contains("no longer applies"));
    assert!(text.contains("knowledge cutoff"));
}

#[tokio::test]
async fn test_generate_disabled_by_config() {
    let mut config = SystemReminderConfig::default();
    config.attachments.model_switch = false;

    assert!(!ModelSwitchGenerator.is_enabled(&config));
}
