use super::*;
use crate::i18n::locale_test_guard;
use crate::state::McpServerOption;
use crate::state::McpServerSelectState;
use crate::state::SkillsDialogState;
use crate::theme::Theme;
use coco_tui_ui::style::UiStyles;

#[test]
fn grouped_list_inserts_group_headers_and_visible_range() {
    #[derive(Debug)]
    struct Item {
        group: &'static str,
    }

    let items = [
        Item { group: "A" },
        Item { group: "A" },
        Item { group: "B" },
        Item { group: "B" },
    ];
    let refs: Vec<&Item> = items.iter().collect();
    let view = grouped_list(&refs, Some(3), 3, |item| item.group);

    assert!(matches!(view.rows[0], PickerRow::Header("A")));
    assert!(matches!(view.rows[3], PickerRow::Blank));
    assert!(matches!(view.rows[4], PickerRow::Header("B")));
    assert_eq!(view.visible, 4..7);
}

#[test]
fn collapse_hints_keeps_output_within_width() {
    let hints = "Up Down  Left Right  Enter Confirm  Esc Cancel";
    assert_eq!(collapse_hints(hints, 80), hints);

    let collapsed = collapse_hints(hints, 20);
    assert!(collapsed.contains("Up Down"));
    assert!(crate::presentation::layout::text_width(&collapsed) <= 20);

    assert_eq!(collapse_hints(hints, 0), "");
}

#[test]
fn skills_dialog_content_renders_flat_list_with_state_and_lock() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();

    // Empty catalog → "no skills" hint, border stays primary.
    let empty = SkillsDialogState::from_wire(coco_types::SkillsDialogPayload {
        entries: Vec::new(),
        bytes_per_token: 4,
    });
    let (title, body, border) = skills_dialog_content(&empty, UiStyles::new(&theme));
    assert_eq!(title, " Skills ");
    assert_eq!(border, theme.primary);
    assert!(body.contains("No skills found."));

    // Mixed catalog: free user skill, plugin-locked skill, off-overridden
    // skill — covers 4-state glyph + lock annotation + plugin footer.
    let payload = coco_types::SkillsDialogPayload {
        entries: vec![
            coco_types::SkillsDialogEntry {
                name: "deploy".into(),
                source: coco_types::SkillsDialogSource::Project,
                description: "Run cargo deploy".into(),
                plugin_name: None,
                frontmatter_bytes: 168,
                current_local: None,
                baseline: coco_types::SkillOverrideState::On,
                lock: None,
            },
            coco_types::SkillsDialogEntry {
                name: "claude-api".into(),
                source: coco_types::SkillsDialogSource::Plugin,
                description: "Anthropic SDK helper".into(),
                plugin_name: Some("claude-plugins-official".into()),
                frontmatter_bytes: 120,
                current_local: None,
                baseline: coco_types::SkillOverrideState::On,
                lock: Some(coco_types::SkillLock {
                    source: coco_types::SkillLockSource::Plugin,
                    forced_value: coco_types::SkillOverrideState::On,
                }),
            },
            coco_types::SkillsDialogEntry {
                name: "noisy".into(),
                source: coco_types::SkillsDialogSource::User,
                description: "loud".into(),
                plugin_name: None,
                frontmatter_bytes: 400,
                current_local: Some(coco_types::SkillOverrideState::Off),
                baseline: coco_types::SkillOverrideState::On,
                lock: None,
            },
        ],
        bytes_per_token: 4,
    };
    let state = SkillsDialogState::from_wire(payload);
    let (_, body, _) = skills_dialog_content(&state, UiStyles::new(&theme));

    // Subtitle includes total + hint.
    assert!(body.contains("3 skills"));
    // Filter placeholder.
    assert!(body.contains("Search skills"));
    // Free row shows state + source + token suffix.
    assert!(body.contains("deploy"));
    // Plugin row carries lock annotation in the locked-by suffix.
    assert!(body.contains("claude-api"));
    assert!(body.contains("locked by plugin"));
    // The off-row shows the "off" label (mirrors `rT5`).
    assert!(body.contains("off"));
    // Plugin footer.
    assert!(body.contains("Plugin skills are managed via /plugin"));
}

#[test]
fn mcp_server_select_content_preserves_checkbox_rows() {
    let _locale = locale_test_guard("en");
    let theme = Theme::default();
    let state = McpServerSelectState {
        servers: vec![
            McpServerOption {
                name: "docs".to_string(),
                selected: true,
                tool_count: 2,
            },
            McpServerOption {
                name: "drive".to_string(),
                selected: false,
                tool_count: 1,
            },
        ],
        filter: "d".to_string(),
    };

    let (title, body, border) = mcp_server_select_content(&state, UiStyles::new(&theme));

    assert_eq!(title, " Select MCP Servers ");
    assert_eq!(border, theme.accent);
    assert!(body.contains("Filter: d"));
    assert!(body.contains("  [x] docs (2 tools)"));
    assert!(body.contains("  [ ] drive (1 tools)"));
}
