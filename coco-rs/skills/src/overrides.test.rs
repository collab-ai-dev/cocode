//! Table-driven tests for the three `skill_overrides` resolvers.
//!
//! Fixture matrix is documented in the V2 plan
//! (`/root/.coco/plans/v2-velvet-engelbart.md` §1.4). Each case
//! pins behavior for one combination of lock-source ×
//! baseline-source × local-override × skill source.

use super::*;
use coco_config::SkillOverrideTiers;
use coco_types::SkillLock;
use coco_types::SkillLockSource;
use coco_types::SkillOverrideState;
use coco_types::SkillOverrideState::{NameOnly, Off, On, UserInvocableOnly};
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::SkillContext;
use crate::SkillDefinition;
use crate::SkillSource;

fn user_skill(name: &str) -> SkillDefinition {
    skill(
        name,
        SkillSource::User {
            path: PathBuf::from("/tmp/skill.md"),
        },
        false,
    )
}

fn plugin_skill(name: &str) -> SkillDefinition {
    skill(
        name,
        SkillSource::Plugin {
            plugin_name: "p".to_string(),
        },
        false,
    )
}

fn mcp_skill(name: &str) -> SkillDefinition {
    skill(
        name,
        SkillSource::Mcp {
            server_name: "acme".to_string(),
        },
        false,
    )
}

fn dmi_skill(name: &str) -> SkillDefinition {
    skill(
        name,
        SkillSource::User {
            path: PathBuf::from("/tmp/skill.md"),
        },
        true,
    )
}

fn skill(name: &str, source: SkillSource, disable_model_invocation: bool) -> SkillDefinition {
    SkillDefinition {
        name: name.into(),
        display_name: None,
        description: String::new(),
        prompt: String::new(),
        source,
        aliases: vec![],
        allowed_tools: None,
        model: None,
        model_role: None,
        when_to_use: None,
        argument_names: vec![],
        paths: vec![],
        effort: None,
        context: SkillContext::Inline,
        agent: None,
        version: None,
        disabled: false,
        hooks: None,
        argument_hint: None,
        user_invocable: true,
        disable_model_invocation,
        shell: None,
        content_length: 0,
        has_user_specified_description: true,
        progress_message: None,
        is_hidden: false,
        gated_by: None,
        files: std::collections::HashMap::new(),
        skill_root: None,
    }
}

#[derive(Default, Debug, Clone)]
struct TierBuilder {
    policy: BTreeMap<String, SkillOverrideState>,
    flag: BTreeMap<String, SkillOverrideState>,
    project: BTreeMap<String, SkillOverrideState>,
    user: BTreeMap<String, SkillOverrideState>,
    local: BTreeMap<String, SkillOverrideState>,
}

impl TierBuilder {
    fn build(self) -> SkillOverrideTiers {
        SkillOverrideTiers {
            policy: self.policy,
            flag: self.flag,
            project: self.project,
            user: self.user,
            local: self.local,
        }
    }
}

fn tier(name: &str, state: SkillOverrideState) -> BTreeMap<String, SkillOverrideState> {
    let mut m = BTreeMap::new();
    m.insert(name.to_string(), state);
    m
}

// ---------- resolve_skill_override_lock ----------

#[test]
fn lock_none_for_user_skill_with_no_override_no_dmi() {
    let s = user_skill("foo");
    let tiers = TierBuilder::default().build();
    assert_eq!(resolve_skill_override_lock(&s, &tiers), None);
}

#[test]
fn lock_policy_wins_over_everything() {
    let s = dmi_skill("foo"); // DMI would otherwise produce author lock
    let tiers = TierBuilder {
        policy: tier("foo", Off),
        flag: tier("foo", On),
        ..Default::default()
    }
    .build();
    assert_eq!(
        resolve_skill_override_lock(&s, &tiers),
        Some(SkillLock {
            source: SkillLockSource::Policy,
            forced_value: Off,
        })
    );
}

#[test]
fn lock_flag_wins_over_author_and_plugin() {
    let s = dmi_skill("foo");
    let tiers = TierBuilder {
        flag: tier("foo", NameOnly),
        ..Default::default()
    }
    .build();
    assert_eq!(
        resolve_skill_override_lock(&s, &tiers),
        Some(SkillLock {
            source: SkillLockSource::Flag,
            forced_value: NameOnly,
        })
    );
}

#[test]
fn lock_author_for_dmi_skill_when_no_policy_or_flag() {
    let s = dmi_skill("foo");
    let tiers = TierBuilder::default().build();
    assert_eq!(
        resolve_skill_override_lock(&s, &tiers),
        Some(SkillLock {
            source: SkillLockSource::Author,
            forced_value: UserInvocableOnly,
        })
    );
}

#[test]
fn lock_plugin_forces_on_when_no_higher_lock() {
    let s = plugin_skill("foo");
    let tiers = TierBuilder {
        // Even a user override is irrelevant — plugin lock wins after
        // policy/flag/author.
        project: tier("foo", Off),
        ..Default::default()
    }
    .build();
    assert_eq!(
        resolve_skill_override_lock(&s, &tiers),
        Some(SkillLock {
            source: SkillLockSource::Plugin,
            forced_value: On,
        })
    );
}

#[test]
fn lock_mcp_skill_has_no_default_lock() {
    let s = mcp_skill("acme:resource");
    let tiers = TierBuilder::default().build();
    // MCP skills don't get a plugin-style "always on" lock —
    // only `source === "plugin"` is special-cased.
    assert_eq!(resolve_skill_override_lock(&s, &tiers), None);
}

// ---------- resolve_skill_baseline ----------

#[test]
fn baseline_defaults_to_on_when_no_tier_has_the_key() {
    let tiers = TierBuilder::default().build();
    assert_eq!(resolve_skill_baseline("foo", &tiers), On);
}

#[test]
fn baseline_reads_project_first() {
    let tiers = TierBuilder {
        project: tier("foo", Off),
        user: tier("foo", NameOnly),
        ..Default::default()
    }
    .build();
    assert_eq!(resolve_skill_baseline("foo", &tiers), Off);
}

#[test]
fn baseline_falls_back_to_user_when_project_absent() {
    let tiers = TierBuilder {
        user: tier("foo", NameOnly),
        ..Default::default()
    }
    .build();
    assert_eq!(resolve_skill_baseline("foo", &tiers), NameOnly);
}

#[test]
fn baseline_ignores_local_policy_flag_layers() {
    // The baseline is the value that resurfaces if the local key is
    // deleted — local / policy / flag must NOT contribute.
    let tiers = TierBuilder {
        policy: tier("foo", Off),
        flag: tier("foo", Off),
        local: tier("foo", Off),
        ..Default::default()
    }
    .build();
    assert_eq!(resolve_skill_baseline("foo", &tiers), On);
}

// ---------- effective_skill_state ----------

#[test]
fn effective_defaults_to_on_when_no_layer_overrides() {
    let s = user_skill("foo");
    let tiers = TierBuilder::default().build();
    assert_eq!(effective_skill_state(&s, &tiers), On);
}

#[test]
fn effective_plugin_source_short_circuits_to_on_regardless_of_overrides() {
    let s = plugin_skill("foo");
    // Even if every tier says "off", a plugin skill stays "on" at
    // runtime — plugin source is checked first.
    let tiers = TierBuilder {
        policy: tier("foo", Off),
        flag: tier("foo", Off),
        local: tier("foo", Off),
        project: tier("foo", Off),
        user: tier("foo", Off),
    }
    .build();
    assert_eq!(effective_skill_state(&s, &tiers), On);
}

#[test]
fn effective_precedence_policy_over_flag_over_local_over_project_over_user() {
    let s = user_skill("foo");
    // user → "off"
    assert_eq!(
        effective_skill_state(
            &s,
            &TierBuilder {
                user: tier("foo", Off),
                ..Default::default()
            }
            .build()
        ),
        Off
    );
    // project beats user
    assert_eq!(
        effective_skill_state(
            &s,
            &TierBuilder {
                project: tier("foo", NameOnly),
                user: tier("foo", Off),
                ..Default::default()
            }
            .build()
        ),
        NameOnly
    );
    // local beats project
    assert_eq!(
        effective_skill_state(
            &s,
            &TierBuilder {
                local: tier("foo", UserInvocableOnly),
                project: tier("foo", NameOnly),
                user: tier("foo", Off),
                ..Default::default()
            }
            .build()
        ),
        UserInvocableOnly
    );
    // flag beats local
    assert_eq!(
        effective_skill_state(
            &s,
            &TierBuilder {
                flag: tier("foo", On),
                local: tier("foo", UserInvocableOnly),
                ..Default::default()
            }
            .build()
        ),
        On
    );
    // policy beats flag
    assert_eq!(
        effective_skill_state(
            &s,
            &TierBuilder {
                policy: tier("foo", Off),
                flag: tier("foo", On),
                ..Default::default()
            }
            .build()
        ),
        Off
    );
}

#[test]
fn effective_does_not_check_disable_model_invocation() {
    // DMI is ignored. The Skill tool runs a separate DMI gate;
    // the listing budget applies XG$ separately. If this function
    // returned UserInvocableOnly for DMI skills with no override,
    // listing budget XG$ would incorrectly drop the row instead of
    // showing the name.
    let s = dmi_skill("foo");
    let tiers = TierBuilder::default().build();
    assert_eq!(effective_skill_state(&s, &tiers), On);
}

#[test]
fn effective_local_can_override_baseline_back_to_on() {
    let s = user_skill("foo");
    // baseline says off (via project), local says on → effective on
    let tiers = TierBuilder {
        project: tier("foo", Off),
        local: tier("foo", On),
        ..Default::default()
    }
    .build();
    assert_eq!(effective_skill_state(&s, &tiers), On);
}

#[test]
fn effective_mcp_skill_respects_overrides_like_user_skill() {
    let s = mcp_skill("acme:resource");
    let tiers = TierBuilder {
        local: tier("acme:resource", Off),
        ..Default::default()
    }
    .build();
    // MCP is not plugin-shortcut → local override applies normally.
    assert_eq!(effective_skill_state(&s, &tiers), Off);
}
