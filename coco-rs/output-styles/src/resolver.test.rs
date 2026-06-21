use super::*;
use crate::builtin::EXPLANATORY_STYLE_NAME;
use crate::catalog::OutputStyleConfig;
use crate::catalog::OutputStyleSource;
use pretty_assertions::assert_eq;

fn style(name: &str, source: OutputStyleSource) -> OutputStyleConfig {
    OutputStyleConfig {
        name: name.to_string(),
        description: name.to_string(),
        prompt: format!("body for {name}"),
        source,
        keep_coding_instructions: None,
        force_for_plugin: None,
    }
}

#[test]
fn aggregate_includes_builtins_by_default() {
    let agg = aggregate(&[], &[]);
    assert!(agg.by_name.contains_key(EXPLANATORY_STYLE_NAME));
    assert!(agg.by_name.contains_key("Learning"));
    // `default` is intentionally NOT in the map — it maps to None.
    assert!(!agg.by_name.contains_key("default"));
}

#[test]
fn dir_priority_wins_over_plugin_priority() {
    let plugin = vec![style("Concise", OutputStyleSource::Plugin)];
    let user = vec![style("Concise", OutputStyleSource::UserSettings)];
    let agg = aggregate(&[user], &plugin);
    assert_eq!(
        agg.by_name["Concise"].source,
        OutputStyleSource::UserSettings
    );
}

#[test]
fn project_overrides_user() {
    let user = vec![style("Concise", OutputStyleSource::UserSettings)];
    let project = vec![style("Concise", OutputStyleSource::ProjectSettings)];
    let agg = aggregate(&[user, project], &[]);
    assert_eq!(
        agg.by_name["Concise"].source,
        OutputStyleSource::ProjectSettings
    );
}

#[test]
fn managed_overrides_project() {
    let project = vec![style("Concise", OutputStyleSource::ProjectSettings)];
    let managed = vec![style("Concise", OutputStyleSource::PolicySettings)];
    let agg = aggregate(&[project, managed], &[]);
    assert_eq!(
        agg.by_name["Concise"].source,
        OutputStyleSource::PolicySettings
    );
}

#[test]
fn resolve_default_returns_none() {
    let agg = aggregate(&[], &[]);
    let (active, verdict) = resolve_active_style(&agg, Some("default"));
    assert!(active.is_none());
    assert_eq!(verdict, ForceForPluginVerdict::None);
}

#[test]
fn resolve_no_settings_value_treated_as_default() {
    let agg = aggregate(&[], &[]);
    let (active, verdict) = resolve_active_style(&agg, None);
    assert!(active.is_none());
    assert_eq!(verdict, ForceForPluginVerdict::None);
}

#[test]
fn resolve_picks_explanatory_when_settings_match() {
    let agg = aggregate(&[], &[]);
    let (active, _) = resolve_active_style(&agg, Some("Explanatory"));
    let active = active.expect("Explanatory should resolve");
    assert_eq!(active.name, "Explanatory");
    assert_eq!(active.source, OutputStyleSource::BuiltIn);
}

#[test]
fn resolve_unknown_name_returns_none() {
    let agg = aggregate(&[], &[]);
    let (active, _) = resolve_active_style(&agg, Some("does-not-exist"));
    assert!(active.is_none());
}

#[test]
fn force_for_plugin_overrides_settings() {
    let mut forced = style("p:forced", OutputStyleSource::Plugin);
    forced.force_for_plugin = Some(true);
    let agg = aggregate(&[], &[forced]);

    let (active, verdict) = resolve_active_style(&agg, Some("Explanatory"));
    let active = active.unwrap();
    assert_eq!(active.name, "p:forced");
    assert_eq!(
        verdict,
        ForceForPluginVerdict::Selected {
            winner: "p:forced".into(),
            competing: vec![]
        }
    );
}

#[test]
fn multiple_force_for_plugin_picks_first_catalog_entry_and_lists_runners_up() {
    let mut a = style("alpha:forced", OutputStyleSource::Plugin);
    a.force_for_plugin = Some(true);
    let mut b = style("beta:forced", OutputStyleSource::Plugin);
    b.force_for_plugin = Some(true);
    let agg = aggregate(&[], &[b, a]);

    let (_, verdict) = resolve_active_style(&agg, Some("default"));
    assert_eq!(
        verdict,
        ForceForPluginVerdict::Selected {
            winner: "beta:forced".into(),
            competing: vec!["alpha:forced".into()]
        }
    );
}
