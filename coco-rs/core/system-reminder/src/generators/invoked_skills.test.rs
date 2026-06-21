use super::*;
use crate::generator::GeneratorContext;
use crate::generator::InvokedSkillEntry;
use coco_config::SystemReminderConfig;

#[tokio::test]
async fn skips_when_empty() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c).invoked_skills(vec![]).build();
    assert!(
        InvokedSkillsGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn emits_with_triple_dash_separator() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .invoked_skills(vec![
            InvokedSkillEntry {
                name: "one".into(),
                path: "/skills/one.md".into(),
                content: "do one thing".into(),
            },
            InvokedSkillEntry {
                name: "two".into(),
                path: "/skills/two.md".into(),
                content: "do two things".into(),
            },
        ])
        .build();
    let text = InvokedSkillsGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    assert!(text.contains("The following skills were invoked in this session"));
    assert!(text.contains("### Skill: one\nPath: /skills/one.md\n\ndo one thing"));
    assert!(text.contains("### Skill: two\nPath: /skills/two.md\n\ndo two things"));
    assert!(text.contains("\n\n---\n\n"));
}
