use super::*;
use coco_types::AgentColorName;
use pretty_assertions::assert_eq;

#[test]
fn yaml_single_quote_doubles_inner_apostrophes() {
    assert_eq!(yaml_single_quote("plain"), "'plain'");
    assert_eq!(yaml_single_quote("it's fine"), "'it''s fine'");
    assert_eq!(yaml_single_quote("a\\b"), "'a\\b'");
}

#[test]
fn template_emits_color_line_when_provided() {
    let body = build_agent_template("Plan", "Plans things.", Some(AgentColorName::Blue));
    assert!(body.contains("name: Plan"));
    assert!(body.contains("description: 'Plans things.'"));
    assert!(body.contains("color: blue"));
}

#[test]
fn template_omits_color_line_when_palette_full() {
    let body = build_agent_template("Plan", "x", None);
    assert!(!body.contains("color:"));
}

#[test]
fn template_round_trips_through_subagent_parser() {
    let body = build_agent_template(
        "demo-agent",
        "Handles when 'edge' cases collide.",
        Some(AgentColorName::Green),
    );
    let parsed = coco_frontmatter::parse(&body);
    let path = std::path::Path::new("/virtual/demo-agent.md");
    let (definition, errors) = coco_subagent::parse_agent_markdown(
        path,
        &parsed.content,
        &parsed.data,
        coco_types::AgentSource::UserSettings,
    )
    .expect("template must parse as a valid agent definition");
    assert!(
        errors.is_empty(),
        "template must parse without validation errors: {errors:?}"
    );
    assert_eq!(definition.name, "demo-agent");
    assert_eq!(
        definition.description.as_deref(),
        Some("Handles when 'edge' cases collide.")
    );
    assert_eq!(definition.color, Some(AgentColorName::Green));
}
