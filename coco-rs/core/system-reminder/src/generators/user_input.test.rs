use super::*;
use crate::generator::GeneratorContext;
use coco_config::SystemReminderConfig;

#[tokio::test]
async fn at_mentioned_files_skips_when_empty() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .at_mentioned_files(vec![])
        .build();
    assert!(
        AtMentionedFilesGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn at_mentioned_files_lists_display_paths() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .at_mentioned_files(vec![
            MentionedFileEntry {
                filename: "/abs/src/lib.rs".into(),
                display_path: "src/lib.rs".into(),
            },
            MentionedFileEntry {
                filename: "/abs/src/main.rs".into(),
                display_path: "src/main.rs".into(),
            },
        ])
        .build();
    let text = AtMentionedFilesGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    assert!(text.contains("The user @-mentioned the following file(s)"));
    assert!(text.contains("- src/lib.rs"));
    assert!(text.contains("- src/main.rs"));
}

#[tokio::test]
async fn mcp_resources_emits_typed_refs() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .mcp_resources(vec![McpResourceEntry {
            server: "fs".into(),
            uri: "file:///tmp/x.txt".into(),
        }])
        .build();
    let text = McpResourcesGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    assert!(text.contains("MCP resource"));
    assert!(text.contains("server=\"fs\""));
    assert!(text.contains("uri=\"file:///tmp/x.txt\""));
}

#[tokio::test]
async fn agent_mentions_emits_per_mention_text() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .agent_mentions(vec![
            AgentMentionEntry {
                agent_type: "explore".into(),
            },
            AgentMentionEntry {
                agent_type: "plan".into(),
            },
        ])
        .build();
    let text = AgentMentionsGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    assert!(text.contains("desire to invoke the agent \"explore\""));
    assert!(text.contains("desire to invoke the agent \"plan\""));
    // Keep a trailing space after each "it." —
    // pin the byte-exact tail so a future refactor can't drop it again.
    assert!(text.contains("passing in the required context to it. "));
}

#[tokio::test]
async fn ide_selection_truncates_long_content() {
    let c = SystemReminderConfig::default();
    let long_content = "x".repeat(2500);
    let ctx = GeneratorContext::builder(&c)
        .ide_selection(Some(IdeSelectionSnapshot {
            filename: "big.rs".into(),
            line_start: 10,
            line_end: 200,
            content: long_content,
        }))
        .build();
    let text = IdeSelectionGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    assert!(text.contains("The user selected the lines 10 to 200 from big.rs"));
    assert!(text.contains("... (truncated)"));
    assert!(text.contains("This may or may not be related to the current task."));
}

#[tokio::test]
async fn ide_opened_file_emits_reminder() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .ide_opened_file(Some(IdeOpenedFileSnapshot {
            filename: "README.md".into(),
        }))
        .build();
    let text = IdeOpenedFileGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    assert!(text.contains("The user opened the file README.md in the IDE"));
}
