use super::*;

fn test_skill(name: &str, desc: &str) -> SkillInfo {
    SkillInfo {
        name: name.to_string(),
        description: Some(desc.to_string()),
        aliases: vec![],
        source: CommandSourceTag::Builtin,
        argument_hint: None,
    }
}

fn sample_skills() -> Vec<SkillInfo> {
    vec![
        test_skill("config", "Show or modify configuration"),
        test_skill("compact", "Compact conversation to reduce context"),
        test_skill("commit", "Create a git commit"),
        test_skill("context", "Show context window usage"),
        test_skill("cost", "Show token usage and cost"),
        test_skill("clear", "Clear conversation history"),
        test_skill("diff", "Show git diff"),
        test_skill("model", "Switch the current model"),
        SkillInfo {
            name: "help".to_string(),
            description: Some("Show available commands".to_string()),
            aliases: vec!["h".to_string(), "?".to_string()],
            source: CommandSourceTag::Builtin,
            argument_hint: Some("[command]".to_string()),
        },
        SkillInfo {
            name: "simplify".to_string(),
            description: Some("Review changed code for quality".to_string()),
            aliases: vec![],
            source: CommandSourceTag::Bundled,
            argument_hint: None,
        },
    ]
}

#[test]
fn test_fuzzy_search_basic() {
    let mgr = SkillSearchManager::new(sample_skills());
    let results = mgr.search("con");

    // Should match config, compact, context, cost (all contain "con")
    assert!(!results.is_empty());
    // First results should be strong matches
    let labels: Vec<&str> = results.iter().map(|r| r.label.as_str()).collect();
    assert!(labels.contains(&"/config"));
    assert!(labels.contains(&"/context"));
}

#[test]
fn test_fuzzy_search_typo() {
    let mgr = SkillSearchManager::new(sample_skills());
    let results = mgr.search("compct");

    // Fuzzy matching should still find "compact"
    assert!(results.iter().any(|r| r.label == "/compact"));
}

#[test]
fn test_search_alias() {
    let mgr = SkillSearchManager::new(sample_skills());
    let results = mgr.search("h");

    assert!(results.iter().any(|r| r.label == "/help [command]"));
}

#[test]
fn name_match_highlight_indices_land_on_the_slash_label() {
    // C6: the name-match indices must be mapped onto the rendered `/name`
    // label — index 0 is the leading slash, so a name hit starts at 1.
    let mgr = SkillSearchManager::new(sample_skills());
    let hit = mgr
        .search("config")
        .into_iter()
        .find(|r| r.label == "/config")
        .expect("config must match");
    // `/config`: c=1 o=2 n=3 f=4 i=5 g=6 — a full-name match highlights 1..=6.
    assert_eq!(hit.highlight_indices, vec![1, 2, 3, 4, 5, 6]);
}

#[test]
fn a_description_only_hit_leaves_the_label_unhighlighted() {
    // "usage" matches the description of `/cost` ("token usage and cost") but
    // nothing in `/cost` itself — marking arbitrary label chars would lie.
    let mgr = SkillSearchManager::new(sample_skills());
    let hit = mgr.search("usage").into_iter().find(|r| r.label == "/cost");
    if let Some(hit) = hit {
        assert!(
            hit.highlight_indices.is_empty(),
            "a description-only hit must not highlight the label"
        );
    }
}

#[test]
fn test_empty_query_returns_all() {
    let mgr = SkillSearchManager::new(sample_skills());
    let results = mgr.search("");
    assert_eq!(results.len(), sample_skills().len());
}

#[test]
fn test_source_annotation() {
    let mgr = SkillSearchManager::new(sample_skills());
    let results = mgr.search("simplify");

    assert!(!results.is_empty());
    let desc = results[0].description.as_deref().unwrap_or("");
    assert!(desc.contains("(bundled)"));
}

#[test]
fn test_max_suggestions_limit() {
    let skills: Vec<SkillInfo> = (0..30)
        .map(|i| test_skill(&format!("skill-{i}"), &format!("Skill number {i}")))
        .collect();
    let mgr = SkillSearchManager::new(skills);
    let results = mgr.search("skill");
    assert!(results.len() <= MAX_SUGGESTIONS);
}

#[test]
fn test_argument_hint_in_label() {
    let mgr = SkillSearchManager::new(sample_skills());
    let results = mgr.search("help");

    let help_result = results.iter().find(|r| r.label.starts_with("/help"));
    assert!(help_result.is_some());
    assert!(help_result.unwrap().label.contains("[command]"));
}
