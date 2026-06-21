use super::*;
use std::fs;

#[tokio::test]
async fn handler_lists_bundled_skills() {
    // No project/user skills — should still list the bundled set.
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    fs::create_dir_all(&cwd).unwrap();
    let config_home = tmp.path().join("home");
    fs::create_dir_all(&config_home).unwrap();

    let out = tokio::task::spawn_blocking({
        let config_home = config_home.clone();
        let cwd = cwd.clone();
        move || render("", &config_home, &cwd)
    })
    .await
    .unwrap()
    .unwrap();

    // Bundled skills are always present (register_bundled_skills runs in
    // build_manager). We only check the shape — skill names drift as new
    // bundled skills land.
    assert!(
        out.contains("skill(s) loaded"),
        "expected count line, got: {out}"
    );
    assert!(
        out.contains("[bundled]"),
        "expected bundled tag, got: {out}"
    );
}

#[tokio::test]
async fn handler_picks_up_project_skill_md() {
    // Drop a SKILL.md into <cwd>/.coco/skills/foo/, expect /skills list
    // to include it tagged as project source.
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    let skill_dir = cwd.join(".coco").join("skills").join("foo");
    fs::create_dir_all(&skill_dir).unwrap();
    // Layout: frontmatter at the top of the file, no
    // leading `# Name` heading. The skill name is taken from the parent
    // directory (`foo`), never from a heading or the frontmatter `name` field.
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\ndescription: a project skill\n---\n\nbody",
    )
    .unwrap();

    let config_home = tmp.path().join("home");
    fs::create_dir_all(&config_home).unwrap();

    let out = tokio::task::spawn_blocking({
        let config_home = config_home.clone();
        let cwd = cwd.clone();
        move || render("", &config_home, &cwd)
    })
    .await
    .unwrap()
    .unwrap();

    assert!(out.contains("/foo"), "expected /foo in output: {out}");
    assert!(out.contains("project"), "expected project tag: {out}");
    assert!(
        out.contains("a project skill"),
        "expected description: {out}"
    );
}

#[tokio::test]
async fn show_unknown_skill_reports_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("p");
    fs::create_dir_all(&cwd).unwrap();
    let config_home = tmp.path().join("h");
    fs::create_dir_all(&config_home).unwrap();

    let out = tokio::task::spawn_blocking({
        let config_home = config_home.clone();
        let cwd = cwd.clone();
        move || render("show no-such-skill", &config_home, &cwd)
    })
    .await
    .unwrap()
    .unwrap();
    assert!(out.contains("No skill named"));
    assert!(out.contains("no-such-skill"));
}

#[tokio::test]
async fn show_without_name_returns_usage() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("p");
    fs::create_dir_all(&cwd).unwrap();
    let config_home = tmp.path().join("h");
    fs::create_dir_all(&config_home).unwrap();

    let out = tokio::task::spawn_blocking({
        let config_home = config_home.clone();
        let cwd = cwd.clone();
        move || render("show", &config_home, &cwd)
    })
    .await
    .unwrap()
    .unwrap();
    assert!(out.contains("Usage: /skills show"));
}

#[tokio::test]
async fn paths_lists_bundled_first() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("p");
    fs::create_dir_all(&cwd).unwrap();
    let config_home = tmp.path().join("h");
    fs::create_dir_all(&config_home).unwrap();

    let out = tokio::task::spawn_blocking({
        let config_home = config_home.clone();
        let cwd = cwd.clone();
        move || render("paths", &config_home, &cwd)
    })
    .await
    .unwrap()
    .unwrap();
    assert!(out.contains("bundled"));
    assert!(out.contains(".coco/skills") || out.contains(".coco\\skills"));
    assert!(!out.contains(".claude/skills") && !out.contains(".claude\\skills"));
}

#[tokio::test]
async fn list_includes_run_hint() {
    // Listing must tell the user how to actually invoke a skill —
    // `/<name>` is the path.
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("p");
    fs::create_dir_all(&cwd).unwrap();
    let config_home = tmp.path().join("h");
    fs::create_dir_all(&config_home).unwrap();

    let out = tokio::task::spawn_blocking({
        let config_home = config_home.clone();
        let cwd = cwd.clone();
        move || render("", &config_home, &cwd)
    })
    .await
    .unwrap()
    .unwrap();
    assert!(
        out.contains("To run a skill"),
        "expected run hint, got: {out}"
    );
}

#[tokio::test]
async fn name_shortcut_resolves_to_show() {
    // `/skills run-skill-generator` should behave like
    // `/skills show run-skill-generator` — analogous to clicking the
    // skill in the menu. Bundled name `run-skill-generator` is stable.
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("p");
    fs::create_dir_all(&cwd).unwrap();
    let config_home = tmp.path().join("h");
    fs::create_dir_all(&config_home).unwrap();

    let bundled_name = "run-skill-generator";
    let direct = tokio::task::spawn_blocking({
        let config_home = config_home.clone();
        let cwd = cwd.clone();
        move || render(bundled_name, &config_home, &cwd)
    })
    .await
    .unwrap()
    .unwrap();
    let via_show = tokio::task::spawn_blocking({
        let config_home = config_home.clone();
        let cwd = cwd.clone();
        move || render(&format!("show {bundled_name}"), &config_home, &cwd)
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(direct, via_show, "shortcut must equal `show <name>` output");
    assert!(direct.contains(&format!("# {bundled_name}")));
}

#[tokio::test]
async fn unknown_subcommand_returns_usage_hint() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("p");
    fs::create_dir_all(&cwd).unwrap();
    let config_home = tmp.path().join("h");
    fs::create_dir_all(&config_home).unwrap();

    let out = tokio::task::spawn_blocking({
        let config_home = config_home.clone();
        let cwd = cwd.clone();
        move || render("foobar", &config_home, &cwd)
    })
    .await
    .unwrap()
    .unwrap();
    assert!(out.contains("Unknown /skills subcommand"));
    assert!(out.contains("Usage"));
}

#[tokio::test]
async fn build_dialog_payload_groups_project_skills_under_project() {
    // Drop a project SKILL.md and verify the dialog payload buckets it
    // under `Project` (proves source-tagging works through `load_scoped`
    // — the earlier `load_from_dirs` flavor would have tagged it as
    // `User`, collapsing the project/user split in the overlay).
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    let skill_dir = cwd.join(".coco").join("skills").join("foo");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\ndescription: a project skill\n---\n\nbody",
    )
    .unwrap();
    let config_home = tmp.path().join("home");
    fs::create_dir_all(&config_home).unwrap();

    let payload = tokio::task::spawn_blocking({
        let config_home = config_home.clone();
        let cwd = cwd.clone();
        move || {
            build_dialog_payload(
                &config_home,
                &cwd,
                &coco_config::SkillOverrideTiers::default(),
            )
        }
    })
    .await
    .unwrap();

    // Project skill `foo` shows up under Project.
    let project_names: Vec<_> = payload
        .entries
        .iter()
        .filter(|e| matches!(e.source, SkillsDialogSource::Project))
        .map(|e| e.name.as_str())
        .collect();
    assert!(
        project_names.contains(&"foo"),
        "expected project skill `foo` in payload, got: {project_names:?}"
    );

    // The dialog ships a flat list (no per-source subtitles).
    // The renderer derives source labels inline from each entry's
    // `source` field, so we only assert the entry surfaced.
    assert!(payload.bytes_per_token > 0);
}

#[tokio::test]
async fn build_dialog_payload_loads_coco_skills_dir_too() {
    // Project discovery is coco-native: `<cwd>/.coco/skills/`.
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    let coco_skill = cwd.join(".coco").join("skills").join("bar");
    fs::create_dir_all(&coco_skill).unwrap();
    fs::write(
        coco_skill.join("SKILL.md"),
        "---\ndescription: a coco-rs project skill\n---\n\nbody",
    )
    .unwrap();
    let config_home = tmp.path().join("home");
    fs::create_dir_all(&config_home).unwrap();

    let payload = tokio::task::spawn_blocking({
        let config_home = config_home.clone();
        let cwd = cwd.clone();
        move || {
            build_dialog_payload(
                &config_home,
                &cwd,
                &coco_config::SkillOverrideTiers::default(),
            )
        }
    })
    .await
    .unwrap();

    let project_names: Vec<_> = payload
        .entries
        .iter()
        .filter(|e| matches!(e.source, SkillsDialogSource::Project))
        .map(|e| e.name.as_str())
        .collect();
    assert!(
        project_names.contains(&"bar"),
        "expected `.coco/skills/bar` in payload as Project, got: {project_names:?}"
    );
}

#[tokio::test]
async fn skills_handler_no_args_opens_dialog() {
    // No-args path emits OpenDialog so the TUI gets a real overlay.
    let h = SkillsHandler;
    let result = h.execute_command("").await.unwrap();
    // Variant shape only — entry contents depend on the process cwd
    // (`SkillsHandler` uses `std::env::current_dir()`), so a content
    // assertion would be flaky across test runners. Content checks
    // live in `build_dialog_payload_*` tests with controlled tmpdirs.
    assert!(
        matches!(
            result,
            CommandResult::OpenDialog(DialogSpec::SkillsList { .. })
        ),
        "expected OpenDialog(SkillsList), got: {result:?}"
    );
}

#[tokio::test]
async fn build_dialog_payload_includes_bundled_skills_as_built_in_source() {
    // Bundled skills surface in the dialog so users
    // can toggle a noisy in-binary skill. Source is `BuiltIn`.
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("empty-project");
    fs::create_dir_all(&cwd).unwrap();
    let config_home = tmp.path().join("empty-home");
    fs::create_dir_all(&config_home).unwrap();

    let payload = tokio::task::spawn_blocking({
        let config_home = config_home.clone();
        let cwd = cwd.clone();
        move || {
            build_dialog_payload(
                &config_home,
                &cwd,
                &coco_config::SkillOverrideTiers::default(),
            )
        }
    })
    .await
    .unwrap();

    let built_in_count = payload
        .entries
        .iter()
        .filter(|e| matches!(e.source, SkillsDialogSource::BuiltIn))
        .count();
    assert!(
        built_in_count > 0,
        "bundled skills must surface as BuiltIn entries; got: {names:?}",
        names = payload
            .entries
            .iter()
            .map(|e| (e.name.as_str(), e.source))
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn skills_handler_with_args_returns_text() {
    // Sub-command path stays text so SDK / scripted callers can parse it.
    let h = SkillsHandler;
    let result = h.execute_command("list").await.unwrap();
    match result {
        CommandResult::Text(out) => {
            assert!(out.contains("skill(s) loaded"), "unexpected text: {out}");
        }
        other => panic!("expected Text, got: {other:?}"),
    }
}

#[tokio::test]
async fn dialog_payload_roundtrip_reflects_persisted_overrides() {
    // Regression for Review Bug 1 + Bug 2: open dialog → user has
    // existing local override + project baseline → enrich payload
    // → assert fields reflect saved state. Without
    // `enrich_payload_with_tiers` being called somewhere, the
    // handler would ship every row with default empty tiers and
    // the dialog would silently render lock-less / baseline=On
    // for skills that actually have overrides on disk.
    use coco_config::SkillOverrideTiers;
    use coco_skills::SkillManager;
    use coco_skills::bundled::register_bundled;
    use coco_types::SkillLockSource;
    use coco_types::SkillOverrideState;
    use std::collections::BTreeMap;

    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    fs::create_dir_all(&cwd).unwrap();
    let config_home = tmp.path().join("home");
    fs::create_dir_all(&config_home).unwrap();

    // Pick a deterministic, lock-free bundled skill so the assertion
    // below is stable. `all_including_conditional` yields `disk.values()`
    // in random per-process HashMap order, and some bundled skills
    // (e.g. `/batch`, `/debug`) carry an Author lock via
    // `disable_model_invocation` — selecting one of those would make
    // `row.lock` non-empty. Filter the locked ones out and sort.
    let bundled_target = {
        let mgr = SkillManager::new();
        register_bundled(&mgr);
        let mut names: Vec<String> = mgr
            .all_including_conditional()
            .iter()
            .filter(|s| !s.disable_model_invocation)
            .map(|s| s.name.clone())
            .collect();
        names.sort();
        names
            .into_iter()
            .next()
            .expect("at least one lock-free bundled skill")
    };

    // Simulate: user previously saved `<target>: off` to localSettings
    // AND project pinned it to `name-only`. (We don't actually write
    // settings.local.json here — just construct the tiers that the
    // RuntimeConfig would expose.)
    let mut local = BTreeMap::new();
    local.insert(bundled_target.clone(), SkillOverrideState::Off);
    let mut project = BTreeMap::new();
    project.insert(bundled_target.clone(), SkillOverrideState::NameOnly);
    let tiers = SkillOverrideTiers {
        local,
        project,
        ..SkillOverrideTiers::default()
    };

    let payload = tokio::task::spawn_blocking({
        let config_home = config_home.clone();
        let cwd = cwd.clone();
        let tiers = tiers.clone();
        move || build_dialog_payload(&config_home, &cwd, &tiers)
    })
    .await
    .unwrap();

    let row = payload
        .entries
        .iter()
        .find(|e| e.name == bundled_target)
        .expect("target skill missing from payload");
    assert_eq!(
        row.current_local,
        Some(SkillOverrideState::Off),
        "current_local must round-trip from tiers.local"
    );
    assert_eq!(
        row.baseline,
        SkillOverrideState::NameOnly,
        "baseline must resolve project ?? user ?? On"
    );
    assert!(row.lock.is_none(), "no policy/flag/author/plugin lock");

    // Now add a policy lock and re-enrich. The CLI bridge always
    // calls `enrich_payload_with_tiers` after `build_dialog_payload`
    // — verify it overwrites stale lock/current_local/baseline.
    let mut policy = BTreeMap::new();
    policy.insert(bundled_target.clone(), SkillOverrideState::Off);
    let locked_tiers = SkillOverrideTiers { policy, ..tiers };
    let mut payload2 = tokio::task::spawn_blocking({
        let config_home = config_home.clone();
        let cwd = cwd.clone();
        let tiers = SkillOverrideTiers::default();
        move || build_dialog_payload(&config_home, &cwd, &tiers)
    })
    .await
    .unwrap();
    // The handler shipped defaults — payload2.entries all have no lock.
    let pre_enrich = payload2
        .entries
        .iter()
        .find(|e| e.name == bundled_target)
        .unwrap();
    assert!(pre_enrich.lock.is_none(), "before enrich: no lock");

    // The CLI bridge step (mirrors tui_runner.rs around the
    // OpenSkillsDialog dispatch).
    let mgr = SkillManager::new();
    register_bundled(&mgr);
    enrich_payload_with_tiers(&mut payload2, &locked_tiers, &mgr);
    let post_enrich = payload2
        .entries
        .iter()
        .find(|e| e.name == bundled_target)
        .unwrap();
    assert_eq!(
        post_enrich.lock.map(|l| l.source),
        Some(SkillLockSource::Policy),
        "enrich_payload_with_tiers must surface the policy lock"
    );
    assert_eq!(
        post_enrich.current_local,
        Some(SkillOverrideState::Off),
        "enrich must populate current_local from tiers"
    );
}
