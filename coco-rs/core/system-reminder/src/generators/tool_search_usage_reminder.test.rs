use super::*;
use crate::generator::GeneratorContext;
use coco_config::SystemReminderConfig;

#[tokio::test]
async fn default_config_is_enabled() {
    let c = SystemReminderConfig::default();
    assert!(ToolSearchUsageReminderGenerator.is_enabled(&c));
}

#[tokio::test]
async fn skips_when_no_deferred_tools() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c).deferred_tools(vec![]).build();
    assert!(
        ToolSearchUsageReminderGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn emits_with_names_when_deferred_present() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .deferred_tools(vec![
            "mcp__slack__send".to_string(),
            "mcp__gh__pr".to_string(),
        ])
        .build();
    let r = ToolSearchUsageReminderGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .expect("emits");
    assert_eq!(r.attachment_type, AttachmentType::ToolSearchUsageReminder);
    let text = r.content().unwrap();
    assert!(text.starts_with(
        "Some available tools' schemas are not loaded in this conversation yet: \
         mcp__slack__send, mcp__gh__pr."
    ));
    assert!(text.contains("use ToolSearch"));
    assert!(text.contains("select:<name>[,<name>...]"));
    assert!(text.contains("gentle reminder - ignore if not applicable to the current work."));
}

#[tokio::test]
async fn collapses_overflow_past_max_listed() {
    let c = SystemReminderConfig::default();
    let many: Vec<String> = (0..MAX_LISTED + 3).map(|i| format!("tool{i}")).collect();
    let ctx = GeneratorContext::builder(&c).deferred_tools(many).build();
    let text = ToolSearchUsageReminderGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    assert!(text.contains("(+3 more)"));
}
