use super::*;
use pretty_assertions::assert_eq;

#[test]
fn defaults_match_registry() {
    let f = Features::with_defaults();
    for spec in all_features() {
        assert_eq!(
            f.enabled(spec.id),
            spec.default_enabled,
            "default mismatch for {:?}",
            spec.id
        );
    }
}

#[test]
fn defaults_for_token_economy_gates() {
    let f = Features::with_defaults();
    assert!(f.enabled(Feature::WebSearch));
    assert!(f.enabled(Feature::WebFetch));
    assert!(f.enabled(Feature::Mcp));
    assert!(f.enabled(Feature::TaskV2));
    assert!(f.enabled(Feature::DynamicModelCard));
    // ToolSearch defaults ON. Sessions with few deferrable tools can opt out
    // via settings.json `features.tool_search = false`.
    assert!(f.enabled(Feature::ToolSearch));
}

#[test]
fn tool_search_key_maps_via_apply_map() {
    let mut f = Features::with_defaults();
    let mut m = BTreeMap::new();
    m.insert("tool_search".to_string(), false);
    f.apply_map(&m);
    assert!(
        !f.enabled(Feature::ToolSearch),
        "explicit `tool_search: false` must turn the gate off"
    );
}

#[test]
fn defaults_for_safety_gate() {
    let f = Features::with_defaults();
    assert!(!f.enabled(Feature::Sandbox), "Sandbox must default off");
}

#[test]
fn plan_mode_defaults_on_and_is_opt_out() {
    let f = Features::with_defaults();
    assert!(
        f.enabled(Feature::PlanMode),
        "PlanMode must default on — it is an opt-out gate"
    );
    let mut m = BTreeMap::new();
    m.insert("plan_mode".to_string(), false);
    let mut f = Features::with_defaults();
    f.apply_map(&m);
    assert!(
        !f.enabled(Feature::PlanMode),
        "explicit `plan_mode: false` must turn the subsystem off"
    );
}

#[test]
fn defaults_for_experimental_gates() {
    let f = Features::with_defaults();
    for feat in [
        Feature::AutoMemory,
        Feature::Retrieval,
        Feature::AgentTeams,
        Feature::Worktree,
        Feature::Lsp,
        Feature::Proactive,
        Feature::KairosBrief,
        Feature::NotebookEdit,
    ] {
        assert!(!f.enabled(feat), "{feat:?} must default off");
    }
}

#[test]
fn speculation_is_experimental_default_off() {
    let f = Features::with_defaults();
    assert!(
        !f.enabled(Feature::Speculation),
        "Speculation must default off (experimental)"
    );
    let info = all_features()
        .find(|s| s.id == Feature::Speculation)
        .unwrap();
    assert_eq!(info.key, "speculation");
    assert!(matches!(info.stage, Stage::Experimental { .. }));
    assert_eq!(
        info.stage.experimental_menu_name(),
        Some("Speculation"),
        "menu name registered"
    );
}

#[test]
fn feature_keys_unique() {
    let mut keys: Vec<&'static str> = all_features().map(|s| s.key).collect();
    keys.sort_unstable();
    let len_before = keys.len();
    keys.dedup();
    assert_eq!(keys.len(), len_before, "feature wire keys must be unique");
}

#[test]
fn empty_starts_with_no_features() {
    let f = Features::empty();
    for spec in all_features() {
        assert!(!f.enabled(spec.id), "empty must not enable {:?}", spec.id);
    }
}

#[test]
fn enable_and_disable_round_trip() {
    let mut f = Features::empty();
    assert!(!f.enabled(Feature::Retrieval));
    f.enable(Feature::Retrieval);
    assert!(f.enabled(Feature::Retrieval));
    f.disable(Feature::Retrieval);
    assert!(!f.enabled(Feature::Retrieval));
}

#[test]
fn apply_map_overrides_registry_defaults() {
    // Start from `with_defaults` so the flip is meaningful — disabling
    // WebSearch (registry default = on) and enabling AutoMemory
    // (registry default = off) both have to land.
    let mut f = Features::with_defaults();
    let mut m = BTreeMap::new();
    m.insert("auto_memory".to_string(), true);
    m.insert("web_search".to_string(), false);
    m.insert("nonsense_unknown_key".to_string(), true);
    f.apply_map(&m);
    assert!(f.enabled(Feature::AutoMemory));
    assert!(!f.enabled(Feature::WebSearch));
    // Untouched defaults stay put.
    assert!(f.enabled(Feature::WebFetch));
}

#[test]
fn feature_for_key_round_trip() {
    for spec in all_features() {
        assert_eq!(feature_for_key(spec.key), Some(spec.id));
    }
    assert_eq!(feature_for_key("not_a_real_key"), None);
}

#[test]
fn is_known_feature_key_truthy_for_registered() {
    assert!(is_known_feature_key("auto_memory"));
    assert!(is_known_feature_key("web_search"));
    assert!(!is_known_feature_key("missing"));
}

#[test]
fn keys_are_unique() {
    let mut seen = std::collections::HashSet::new();
    for spec in all_features() {
        assert!(seen.insert(spec.key), "duplicate key: {}", spec.key);
    }
}

#[test]
fn ids_are_unique() {
    let mut seen = std::collections::HashSet::new();
    for spec in all_features() {
        assert!(seen.insert(spec.id), "duplicate id: {:?}", spec.id);
    }
}

#[test]
fn enabled_features_is_sorted_and_dedup() {
    let mut f = Features::empty();
    f.enable(Feature::Retrieval);
    f.enable(Feature::AutoMemory);
    f.enable(Feature::AutoMemory);
    let v = f.enabled_features();
    assert_eq!(v.len(), 2);
    assert!(v.windows(2).all(|w| w[0] < w[1]));
}
