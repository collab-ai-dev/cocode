use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::HashMap;

fn per_source_with(source: SettingSource, value: Value) -> HashMap<SettingSource, Value> {
    let mut m = HashMap::new();
    m.insert(source, value);
    m
}

#[test]
fn from_per_source_empty_when_no_tier_has_skill_overrides_key() {
    let per_source = per_source_with(SettingSource::User, json!({ "language": "en" }));
    let tiers = SkillOverrideTiers::from_per_source(&per_source);
    assert!(tiers.is_empty());
}

#[test]
fn from_per_source_extracts_each_tier_independently() {
    let mut per_source = HashMap::new();
    per_source.insert(
        SettingSource::Policy,
        json!({ "skill_overrides": { "noisy": "off" } }),
    );
    per_source.insert(
        SettingSource::Project,
        json!({ "skill_overrides": { "team-skill": "name-only" } }),
    );
    per_source.insert(
        SettingSource::User,
        json!({ "skill_overrides": { "global-skill": "user-invocable-only" } }),
    );
    per_source.insert(
        SettingSource::Local,
        json!({ "skill_overrides": { "noisy": "on" } }),
    );

    let tiers = SkillOverrideTiers::from_per_source(&per_source);

    assert_eq!(tiers.policy.get("noisy"), Some(&SkillOverrideState::Off));
    assert_eq!(
        tiers.project.get("team-skill"),
        Some(&SkillOverrideState::NameOnly)
    );
    assert_eq!(
        tiers.user.get("global-skill"),
        Some(&SkillOverrideState::UserInvocableOnly)
    );
    // `local` carries the diff against baseline — local says "on"
    // even though policy locks to "off". Resolution wires policy on
    // top; here we're just asserting the raw extraction preserves
    // tiers without merging.
    assert_eq!(tiers.local.get("noisy"), Some(&SkillOverrideState::On));
}

#[test]
fn from_per_source_drops_malformed_state_strings_silently() {
    let per_source = per_source_with(
        SettingSource::User,
        json!({ "skill_overrides": { "good": "off", "bad": "garbage", "ok": "name-only" } }),
    );
    let tiers = SkillOverrideTiers::from_per_source(&per_source);
    assert_eq!(tiers.user.get("good"), Some(&SkillOverrideState::Off));
    assert_eq!(tiers.user.get("ok"), Some(&SkillOverrideState::NameOnly));
    assert!(!tiers.user.contains_key("bad"));
}

#[test]
fn from_per_source_drops_non_object_skill_overrides_value() {
    let per_source = per_source_with(
        SettingSource::Project,
        json!({ "skill_overrides": "not-an-object" }),
    );
    let tiers = SkillOverrideTiers::from_per_source(&per_source);
    assert!(tiers.project.is_empty());
}

#[test]
fn plugin_tier_intentionally_not_extracted() {
    // Plugin lock comes from the skill's `SkillSource::Plugin`, not
    // from a settings tier. Even if a plugin contributed a
    // `skill_overrides` block, this resolver ignores it — preserves
    // the TS contract (see `oT5` which has no plugin tier read).
    let mut per_source = HashMap::new();
    per_source.insert(
        SettingSource::Plugin,
        json!({ "skill_overrides": { "foo": "off" } }),
    );
    let tiers = SkillOverrideTiers::from_per_source(&per_source);
    assert!(tiers.is_empty());
}
