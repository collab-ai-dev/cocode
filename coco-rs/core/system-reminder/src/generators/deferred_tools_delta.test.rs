use super::*;
use crate::generator::DeferredToolsDeltaInfo;
use crate::generator::GeneratorContext;
use coco_config::SystemReminderConfig;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn default_config_is_enabled() {
    let c = SystemReminderConfig::default();
    assert!(DeferredToolsDeltaGenerator.is_enabled(&c));
}

#[tokio::test]
async fn skips_when_no_delta() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .deferred_tools_delta(None)
        .build();
    assert!(
        DeferredToolsDeltaGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn skips_when_delta_empty() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .deferred_tools_delta(Some(DeferredToolsDeltaInfo::default()))
        .build();
    assert!(
        DeferredToolsDeltaGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn emits_added_only() {
    let c = SystemReminderConfig::default();
    let info = DeferredToolsDeltaInfo {
        added_lines: vec!["- Foo: Does foo".to_string(), "- Bar: Does bar".to_string()],
        removed_names: vec![],
    };
    let ctx = GeneratorContext::builder(&c)
        .deferred_tools_delta(Some(info))
        .build();
    let r = DeferredToolsDeltaGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .expect("emits");
    assert_eq!(r.attachment_type, AttachmentType::DeferredToolsDelta);
    let text = r.content().unwrap();
    assert_eq!(
        text,
        "The following deferred tools are now available via ToolSearch. Their schemas are NOT loaded — calling them directly will fail with InputValidationError. Use ToolSearch with query \"select:<name>[,<name>...]\" to load tool schemas before calling them:\n- Foo: Does foo\n- Bar: Does bar"
    );
}

#[tokio::test]
async fn emits_removed_only() {
    let c = SystemReminderConfig::default();
    let info = DeferredToolsDeltaInfo {
        added_lines: vec![],
        removed_names: vec!["OldTool".to_string()],
    };
    let ctx = GeneratorContext::builder(&c)
        .deferred_tools_delta(Some(info))
        .build();
    let r = DeferredToolsDeltaGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .expect("emits");
    let text = r.content().unwrap();
    assert!(text.starts_with("The following deferred tools are no longer available"));
    assert!(text.contains("OldTool"));
    assert!(text.contains("ToolSearch will return no match"));
}

#[tokio::test]
async fn emits_both_sections_joined_by_blank_line() {
    let c = SystemReminderConfig::default();
    let info = DeferredToolsDeltaInfo {
        added_lines: vec!["- NewTool: Does new".to_string()],
        removed_names: vec!["OldTool".to_string()],
    };
    let ctx = GeneratorContext::builder(&c)
        .deferred_tools_delta(Some(info))
        .build();
    let text = DeferredToolsDeltaGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    // Two sections joined by \n\n
    assert!(text.contains("available via ToolSearch"));
    assert!(text.contains("schemas are NOT loaded"));
    assert!(text.contains("select:<name>"));
    assert!(text.contains("no longer available"));
    assert!(text.contains("- NewTool: Does new\n\n"));
}
