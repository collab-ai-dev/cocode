use super::*;
use coco_types::AgentDefinition;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn def(name: &str, source: AgentSource, color: Option<AgentColorName>) -> AgentDefinition {
    AgentDefinition {
        name: name.into(),
        source,
        color,
        ..AgentDefinition::default()
    }
}

fn snapshot(defs: Vec<AgentDefinition>) -> AgentCatalogSnapshot {
    let active: BTreeMap<String, AgentDefinition> =
        defs.iter().map(|d| (d.name.clone(), d.clone())).collect();
    let all = defs
        .into_iter()
        .map(|d| crate::definition_store::LoadedAgentDefinition {
            definition: d,
            path: None,
        })
        .collect();
    AgentCatalogSnapshot::new(active, all)
}

#[test]
fn user_source_resolves_under_config_home() {
    let dir = resolve_writable_agent_dir(
        AgentSource::UserSettings,
        &PathBuf::from("/home/u/.coco"),
        &PathBuf::from("/tmp/proj"),
    );
    assert_eq!(dir, Some(PathBuf::from("/home/u/.coco/agents")));
}

#[test]
fn project_source_resolves_under_cwd_coco() {
    let dir = resolve_writable_agent_dir(
        AgentSource::ProjectSettings,
        &PathBuf::from("/home/u/.coco"),
        &PathBuf::from("/tmp/proj"),
    );
    assert_eq!(dir, Some(PathBuf::from("/tmp/proj/.coco/agents")));
}

#[test]
fn non_writable_sources_return_none() {
    for source in [
        AgentSource::BuiltIn,
        AgentSource::Plugin,
        AgentSource::FlagSettings,
        AgentSource::PolicySettings,
    ] {
        assert_eq!(
            resolve_writable_agent_dir(source, &PathBuf::from("/cfg"), &PathBuf::from("/cwd"),),
            None,
            "{source:?} should not be writable"
        );
    }
}

#[test]
fn next_color_picks_first_palette_entry_when_empty() {
    let snap = snapshot(vec![]);
    assert_eq!(next_unused_color(&snap), Some(AgentColorName::Red));
}

#[test]
fn next_color_skips_used_entries() {
    let snap = snapshot(vec![
        def("a", AgentSource::UserSettings, Some(AgentColorName::Red)),
        def("b", AgentSource::UserSettings, Some(AgentColorName::Blue)),
    ]);
    assert_eq!(next_unused_color(&snap), Some(AgentColorName::Green));
}

#[test]
fn next_color_cycles_when_palette_full() {
    // Once every palette entry is used at least once, the picker
    // falls back to cycling by active-agent count so the new agent
    // still gets a colour. Eight active agents → index 8 % 8 = 0,
    // which lands back on the first palette entry (Red).
    let all_colors: Vec<AgentDefinition> = AgentColorName::ALL
        .iter()
        .enumerate()
        .map(|(i, c)| def(&format!("a{i}"), AgentSource::UserSettings, Some(*c)))
        .collect();
    let snap = snapshot(all_colors);
    assert_eq!(next_unused_color(&snap), Some(AgentColorName::Red));
}
