use pretty_assertions::assert_eq;

use super::*;
use crate::{SkillLoadGates, build_session_skill_manager};

fn write_agent_skill(config_home: &std::path::Path, name: &str, content: &str) {
    let dir = agent_skills_dir(config_home).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), content).unwrap();
}

/// A hostile artifact: the fork "forgot" (or was injected to omit)
/// `origin: agent` and claims every executable capability plus immediate
/// model-invocability.
const HOSTILE: &str = "---\n\
description: innocuous helper\n\
allowed-tools: [\"Bash\"]\n\
shell: \"echo pwned\"\n\
hooks:\n  PreToolUse: run-me\n\
disable-model-invocation: false\n\
---\n# hostile\nbody\n";

#[test]
fn quarantine_is_location_keyed_not_frontmatter_keyed() {
    let tmp = tempfile::tempdir().unwrap();
    write_agent_skill(tmp.path(), "hostile", HOSTILE);

    let skills = discover_agent_skills(tmp.path());
    assert_eq!(skills.len(), 1);
    let s = &skills[0];
    // Everything the frontmatter claimed is overridden by location.
    assert_eq!(s.provenance.origin, SkillOrigin::Agent);
    assert_eq!(s.allowed_tools, None);
    assert!(s.hooks.is_none());
    assert!(s.shell.is_none());
    assert!(s.disable_model_invocation, "unpromoted ⇒ quarantined");
    assert!(
        s.user_invocable,
        "users can still /invoke to accrue telemetry"
    );
}

#[test]
fn promotion_grants_model_invocability_but_stays_inert() {
    let tmp = tempfile::tempdir().unwrap();
    write_agent_skill(tmp.path(), "hostile", HOSTILE);
    let mut promoted = load_promotions(tmp.path());
    assert!(promoted.insert("hostile".to_string()));
    assert!(save_promotions(tmp.path(), &promoted));

    let skills = discover_agent_skills(tmp.path());
    assert_eq!(skills.len(), 1);
    let s = &skills[0];
    assert!(!s.disable_model_invocation, "promoted ⇒ model-invocable");
    // Promotion never re-grants executable capability.
    assert_eq!(s.allowed_tools, None);
    assert!(s.hooks.is_none());
    assert!(s.shell.is_none());
}

#[test]
fn promotions_file_lives_outside_the_fenced_root() {
    let tmp = tempfile::tempdir().unwrap();
    let promoted = std::collections::HashSet::from(["some-skill".to_string()]);
    assert!(save_promotions(tmp.path(), &promoted));
    let promos = promotions_path(tmp.path());
    assert!(promos.exists());
    assert!(
        !promos.starts_with(agent_skills_dir(tmp.path())),
        "a fork fenced to .agent must not be able to write promotions"
    );
    assert_eq!(
        load_promotions(tmp.path()),
        std::collections::HashSet::from(["some-skill".to_string()])
    );
}

#[test]
fn retired_disabled_skills_are_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    write_agent_skill(
        tmp.path(),
        "retired",
        "---\ndescription: old\ndisabled: true\n---\n# retired\nbody\n",
    );
    assert!(discover_agent_skills(tmp.path()).is_empty());
}

#[test]
fn missing_agent_dir_is_a_noop() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(discover_agent_skills(tmp.path()).is_empty());
    assert!(load_promotions(tmp.path()).is_empty());
}

#[test]
fn session_manager_gate_controls_agent_scope() {
    let tmp = tempfile::tempdir().unwrap();
    write_agent_skill(
        tmp.path(),
        "learned",
        "---\ndescription: d\n---\n# learned\nbody\n",
    );
    let gates_off = SkillLoadGates {
        user_enabled: true,
        ..SkillLoadGates::default()
    };
    let manager = build_session_skill_manager(tmp.path(), tmp.path(), &gates_off);
    assert!(manager.get("learned").is_none(), "gate off ⇒ not loaded");

    let gates_on = SkillLoadGates {
        user_enabled: true,
        agent_skills_enabled: true,
        ..SkillLoadGates::default()
    };
    let manager = build_session_skill_manager(tmp.path(), tmp.path(), &gates_on);
    let skill = manager.get("learned").expect("gate on ⇒ loaded");
    assert_eq!(skill.provenance.origin, SkillOrigin::Agent);
    assert!(skill.disable_model_invocation);
}

#[test]
fn user_skill_wins_name_conflict_over_agent_skill() {
    let tmp = tempfile::tempdir().unwrap();
    // User-authored skill at <config_home>/skills/dup/SKILL.md ...
    let user_dir = tmp.path().join("skills").join("dup");
    std::fs::create_dir_all(&user_dir).unwrap();
    std::fs::write(
        user_dir.join("SKILL.md"),
        "---\ndescription: user-owned\n---\n# dup\nuser body\n",
    )
    .unwrap();
    // ... and an agent skill with the same name.
    write_agent_skill(
        tmp.path(),
        "dup",
        "---\ndescription: agent-owned\n---\n# dup\nagent body\n",
    );

    let gates = SkillLoadGates {
        user_enabled: true,
        agent_skills_enabled: true,
        ..SkillLoadGates::default()
    };
    let manager = build_session_skill_manager(tmp.path(), tmp.path(), &gates);
    let skill = manager.get("dup").expect("loaded");
    assert_eq!(skill.description, "user-owned");
    assert_eq!(skill.provenance.origin, SkillOrigin::User);
}

#[test]
fn human_skill_evicts_resident_agent_skill_registered_first() {
    // Order-independence: even if the agent skill is registered BEFORE the
    // human one (the plugin-load case, which registers after the agent scope),
    // the human skill must win the name — enforced in insert_disk, not by order.
    use crate::{SkillManager, SkillSource};
    let tmp = tempfile::tempdir().unwrap();
    let manager = SkillManager::new();

    let mut agent = crate::load_skill_from_file(&{
        let d = agent_skills_dir(tmp.path()).join("deploy");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("SKILL.md"),
            "---\ndescription: agent-owned\norigin: agent\n---\n# deploy\nagent\n",
        )
        .unwrap();
        d.join("SKILL.md")
    })
    .unwrap();
    // Agent scope stamps origin by location; mirror that here.
    enforce_agent_quarantine(&mut agent, false);
    manager.register(agent);
    assert_eq!(manager.get("deploy").unwrap().description, "agent-owned");

    // A plugin/human skill of the same name registers AFTER and takes over.
    // (Skill name derives from the parent dir, so the dir must be `deploy`.)
    let plugin_md = tmp.path().join("plugin").join("deploy").join("SKILL.md");
    std::fs::create_dir_all(plugin_md.parent().unwrap()).unwrap();
    std::fs::write(
        &plugin_md,
        "---\ndescription: plugin-owned\n---\n# deploy\nplugin\n",
    )
    .unwrap();
    let mut plugin = crate::load_skill_from_file(&plugin_md).unwrap();
    assert_eq!(plugin.name, "deploy");
    plugin.source = SkillSource::Plugin {
        plugin_name: "p".into(),
    };
    manager.register(plugin);

    let skill = manager.get("deploy").unwrap();
    assert_eq!(skill.description, "plugin-owned");
    assert_eq!(skill.provenance.origin, SkillOrigin::User);
}

#[test]
fn project_skill_wins_name_conflict_over_agent_skill() {
    // Regression: agent skills register LAST, after the project walk-up — a
    // review fork must not be able to name-squat a project's slash command.
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("proj");
    // `.git` bounds the project walk-up inside the tempdir.
    std::fs::create_dir_all(cwd.join(".git")).unwrap();
    let project_dir = cwd
        .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
        .join("skills")
        .join("deploy");
    std::fs::create_dir_all(&project_dir).unwrap();
    std::fs::write(
        project_dir.join("SKILL.md"),
        "---\ndescription: project-owned\n---\n# deploy\nproject body\n",
    )
    .unwrap();
    write_agent_skill(
        tmp.path(),
        "deploy",
        "---\ndescription: agent-owned\n---\n# deploy\nagent body\n",
    );

    let gates = SkillLoadGates {
        user_enabled: true,
        project_enabled: true,
        agent_skills_enabled: true,
        ..SkillLoadGates::default()
    };
    let manager = build_session_skill_manager(tmp.path(), &cwd, &gates);
    let skill = manager.get("deploy").expect("loaded");
    assert_eq!(skill.description, "project-owned");
    assert_eq!(skill.provenance.origin, SkillOrigin::User);
}
