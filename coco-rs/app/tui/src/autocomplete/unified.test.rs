use super::*;
use crate::widgets::suggestion_popup::SuggestionMeta;
use coco_types::AgentColorName;
use pretty_assertions::assert_eq;

fn agent(name: &str, color: Option<AgentColorName>) -> AgentInfo {
    AgentInfo {
        name: name.into(),
        description: Some(format!("{name} description")),
        color,
    }
}

fn file_item(path: &str) -> SuggestionItem {
    SuggestionItem {
        label: path.into(),
        description: None,
        metadata: Some(SuggestionMeta::Path {
            is_directory: false,
        }),
    }
}

#[test]
fn seed_agent_items_appends_agent_suffix_to_label() {
    let agents = vec![agent("Plan", Some(AgentColorName::Blue))];
    let items = seed_agent_items(&agents, "");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].label, "Plan (agent)");
    match &items[0].metadata {
        Some(SuggestionMeta::Agent { color }) => assert_eq!(*color, Some(AgentColorName::Blue)),
        other => panic!("expected Agent metadata, got {other:?}"),
    }
}

#[test]
fn seed_agent_items_substring_filters_case_insensitively() {
    let agents = vec![
        agent("Plan", None),
        agent("Explore", None),
        agent("general-purpose", None),
    ];
    let items = seed_agent_items(&agents, "EX");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].label, "Explore (agent)");
}

#[test]
fn merge_keeps_agents_before_files_and_caps_at_15() {
    let agents: Vec<SuggestionItem> = (0..5)
        .map(|i| SuggestionItem {
            label: format!("a{i} (agent)"),
            description: None,
            metadata: Some(SuggestionMeta::Agent { color: None }),
        })
        .collect();
    let files: Vec<SuggestionItem> = (0..20).map(|i| file_item(&format!("file{i}.rs"))).collect();
    let merged = merge_file_results(agents, files);
    assert_eq!(merged.len(), 15);
    // first 5 are agents
    assert!(
        merged[..5]
            .iter()
            .all(|s| matches!(s.metadata, Some(SuggestionMeta::Agent { .. })))
    );
    // remaining 10 are files (cap eats the rest)
    assert!(
        merged[5..]
            .iter()
            .all(|s| matches!(s.metadata, Some(SuggestionMeta::Path { .. })))
    );
}

#[test]
fn seeded_provider_merge_reserves_room_for_mcp_when_agents_fill_cap() {
    let agents: Vec<SuggestionItem> = (0..20)
        .map(|i| SuggestionItem {
            label: format!("a{i} (agent)"),
            description: None,
            metadata: Some(SuggestionMeta::Agent { color: None }),
        })
        .collect();
    let resources = vec![SuggestionItem {
        label: "Guide".into(),
        description: None,
        metadata: Some(SuggestionMeta::McpResource {
            server: "docs".into(),
            uri: "file://guide".into(),
        }),
    }];

    let merged = merge_seeded_provider_items(agents, resources);

    assert_eq!(merged.len(), 15);
    assert!(matches!(
        merged.last().and_then(|item| item.metadata.as_ref()),
        Some(SuggestionMeta::McpResource { server, uri })
            if server == "docs" && uri == "file://guide"
    ));
}
