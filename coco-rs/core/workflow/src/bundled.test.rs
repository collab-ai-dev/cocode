use pretty_assertions::assert_eq;

use super::*;
use crate::MAX_WORKFLOW_SOURCE_BYTES;

/// Registry construction drops a script whose meta fails to parse, so without
/// this test a malformed bundled workflow would ship as a *missing* one.
#[test]
fn test_bundled_scripts_all_parse_into_the_registry() {
    assert_eq!(bundled_workflows().len(), BUNDLED_SCRIPTS.len());
}

/// The bundled scripts are launched through the same path as user scripts, so
/// they must clear every gate that path applies.
#[test]
fn test_bundled_scripts_are_deterministic_and_within_the_size_cap() {
    for script in BUNDLED_SCRIPTS {
        parse_workflow_script(script, /*check_determinism*/ true)
            .expect("bundled script must pass the launch-time determinism check");
        assert!(script.len() <= MAX_WORKFLOW_SOURCE_BYTES);
    }
}

/// Lookup is by name, so duplicates would make one script unreachable.
#[test]
fn test_bundled_workflow_names_are_unique() {
    let mut names: Vec<&str> = bundled_workflows()
        .iter()
        .map(|workflow| workflow.meta.name.as_str())
        .collect();
    let total = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), total);
}

#[test]
fn test_deep_research_meta_matches_its_declared_pipeline() {
    let deep_research = bundled_workflow("deep-research").expect("deep-research is bundled");
    let phases: Vec<&str> = deep_research
        .meta
        .phases
        .iter()
        .map(|phase| phase.title.as_str())
        .collect();
    assert_eq!(phases, ["Scope", "Search", "Fetch", "Verify", "Synthesize"]);
    // `whenToUse` is what the projected slash command shows the model before it
    // spends ~100 subagents; an empty one silently drops the cost disclosure.
    assert!(
        deep_research
            .meta
            .when_to_use
            .as_deref()
            .is_some_and(|text| !text.trim().is_empty())
    );
    assert!(!deep_research.meta.description.trim().is_empty());
}

#[test]
fn test_bundled_workflow_lookup_misses_unknown_name() {
    assert!(bundled_workflow("not-a-bundled-workflow").is_none());
}
