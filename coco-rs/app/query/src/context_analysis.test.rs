use super::*;

#[test]
fn format_markdown_renders_source_detail_sections() {
    let report = ContextUsageReport {
        total_tokens: 100,
        max_tokens: 1_000,
        raw_max_tokens: 1_000,
        percentage: 10.0,
        model: ProviderModelSelection {
            provider: "test".into(),
            model_id: "model".into(),
        },
        categories: vec![
            ContextUsageCategory {
                kind: ContextCategoryKind::Agents,
                tokens: 7,
            },
            ContextUsageCategory {
                kind: ContextCategoryKind::Skills,
                tokens: 5,
            },
            ContextUsageCategory {
                kind: ContextCategoryKind::Free,
                tokens: 900,
            },
        ],
        memory_files: vec![MemoryFileEstimate {
            path: "/tmp/AGENTS.md".into(),
            source: "Project".into(),
            tokens: 11,
        }],
        mcp_tools: vec![McpToolEstimate {
            name: "search".into(),
            server_name: "github".into(),
            tokens: 13,
            deferred: true,
        }],
        agents: vec![AgentEstimate {
            agent_type: "reviewer".into(),
            source: "projectSettings".into(),
            tokens: 7,
        }],
        skills: vec![SkillEstimate {
            name: "rust-review".into(),
            source: "user:/skills/rust-review".into(),
            tokens: 5,
        }],
        message_breakdown: coco_types::MessageBreakdown {
            tool_call_tokens: 0,
            tool_result_tokens: 0,
            attachment_tokens: 0,
            assistant_message_tokens: 0,
            user_message_tokens: 0,
            tool_calls_by_type: Vec::new(),
            attachments_by_type: Vec::new(),
        },
        is_auto_compact_enabled: false,
        auto_compact_threshold: None,
    };

    let markdown = format_markdown(&report);

    assert!(markdown.contains("| Agent Type | Source | Tokens |"));
    assert!(markdown.contains("| reviewer | projectSettings | 7 |"));
    assert!(markdown.contains("| Skill | Source | Tokens |"));
    assert!(markdown.contains("| rust-review | user:/skills/rust-review | 5 |"));
    assert!(markdown.contains("| search | github | 13 | deferred |"));
}

#[test]
fn format_markdown_renders_block_grid_and_token_units() {
    let report = ContextUsageReport {
        total_tokens: 44_000,
        max_tokens: 1_000_000,
        raw_max_tokens: 1_000_000,
        percentage: 4.4,
        model: ProviderModelSelection {
            provider: "anthropic".into(),
            model_id: "claude".into(),
        },
        categories: vec![
            ContextUsageCategory {
                kind: ContextCategoryKind::Tools,
                tokens: 14_000,
            },
            ContextUsageCategory {
                kind: ContextCategoryKind::Messages,
                tokens: 13_900,
            },
            ContextUsageCategory {
                kind: ContextCategoryKind::Free,
                tokens: 956_000,
            },
        ],
        memory_files: vec![],
        mcp_tools: vec![],
        agents: vec![],
        skills: vec![],
        message_breakdown: coco_types::MessageBreakdown {
            tool_call_tokens: 0,
            tool_result_tokens: 0,
            attachment_tokens: 0,
            assistant_message_tokens: 0,
            user_message_tokens: 0,
            tool_calls_by_type: Vec::new(),
            attachments_by_type: Vec::new(),
        },
        is_auto_compact_enabled: false,
        auto_compact_threshold: None,
    };

    let md = format_markdown(&report);

    // Block grid (free + used glyphs present).
    assert!(md.contains(GLYPH_FREE));
    assert!(md.contains(GLYPH_USED));
    // Category labels + `tok`/`k` units.
    assert!(md.contains("System tools: 14k tok"));
    assert!(md.contains("Messages: 13.9k tok"));
    assert!(md.contains("Free space:"));
    // Headline uses compact units and integer percent.
    assert!(md.contains("44k/1m tok (4%)"));
    // Legacy estimate table is gone.
    assert!(!md.contains("| Category | Tokens | Pct |"));
}

#[test]
fn context_usage_category_labels_are_stable() {
    assert_eq!(ContextCategoryKind::SystemPrompt.label(), "System prompt");
    assert_eq!(ContextCategoryKind::Tools.label(), "System tools");
    assert_eq!(ContextCategoryKind::McpTools.label(), "MCP tools");
    assert_eq!(ContextCategoryKind::Agents.label(), "Custom agents");
    assert_eq!(ContextCategoryKind::MemoryFiles.label(), "Memory files");
    assert_eq!(ContextCategoryKind::Skills.label(), "Skills");
    assert_eq!(ContextCategoryKind::Messages.label(), "Messages");
    assert_eq!(ContextCategoryKind::Free.label(), "Free space");
}
