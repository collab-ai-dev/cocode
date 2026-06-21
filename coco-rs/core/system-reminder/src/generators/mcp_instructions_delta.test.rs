use super::*;
use crate::generator::GeneratorContext;
use crate::generator::McpInstructionsDeltaInfo;
use coco_config::SystemReminderConfig;

#[tokio::test]
async fn skips_when_no_delta() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .mcp_instructions_delta(None)
        .build();
    assert!(
        McpInstructionsDeltaGenerator
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
        .mcp_instructions_delta(Some(McpInstructionsDeltaInfo::default()))
        .build();
    assert!(
        McpInstructionsDeltaGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn emits_added_section_with_header() {
    let c = SystemReminderConfig::default();
    let info = McpInstructionsDeltaInfo {
        added_blocks: vec![
            "## linear\n\nUse this to manage tickets.".to_string(),
            "## github\n\nUse this for PR interactions.".to_string(),
        ],
        removed_names: vec![],
    };
    let ctx = GeneratorContext::builder(&c)
        .mcp_instructions_delta(Some(info))
        .build();
    let text = McpInstructionsDeltaGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    assert!(text.starts_with("# MCP Server Instructions"));
    assert!(text.contains("The following MCP servers have provided instructions"));
    assert!(text.contains("## linear"));
    assert!(text.contains("## github"));
    // Server blocks joined by \n\n.
    assert!(text.contains("Use this to manage tickets.\n\n## github"));
}

#[tokio::test]
async fn emits_removed_section() {
    let c = SystemReminderConfig::default();
    let info = McpInstructionsDeltaInfo {
        added_blocks: vec![],
        removed_names: vec!["linear".to_string(), "jira".to_string()],
    };
    let ctx = GeneratorContext::builder(&c)
        .mcp_instructions_delta(Some(info))
        .build();
    let text = McpInstructionsDeltaGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    assert!(text.starts_with("The following MCP servers have disconnected"));
    assert!(text.contains("linear\njira"));
}
