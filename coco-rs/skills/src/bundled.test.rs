use super::*;
use coco_types::Features;
use coco_types::ToolName;

#[test]
fn catalog_includes_formerly_ant_skills() {
    // coco-rs drops the `USER_TYPE === 'ant'` gate: these general-purpose
    // skills are available to every user, alongside the always-on set.
    let skills = get_bundled_skills();
    let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
    for required in [
        "update-config",
        "keybindings-help",
        "batch",
        "verify",
        "debug",
        "skillify",
        "remember",
        "simplify",
        "stuck",
        "lorem-ipsum",
    ] {
        assert!(
            names.contains(&required),
            "bundled catalog should include {required}"
        );
    }
}

#[test]
fn no_rust_only_extras() {
    // commit/review-pr/pdf were removed in Round 11 — not shipped as bundled skills.
    let skills = get_bundled_skills();
    let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
    assert!(!names.contains(&"commit"));
    assert!(!names.contains(&"review-pr"));
    assert!(!names.contains(&"pdf"));
}

#[test]
fn debug_disables_model_invocation() {
    let skills = get_bundled_skills();
    let debug = skills.iter().find(|s| s.name == "debug").unwrap();
    assert!(debug.disable_model_invocation);
}

#[test]
fn batch_disables_model_invocation() {
    let skills = get_bundled_skills();
    let batch = skills.iter().find(|s| s.name == "batch").unwrap();
    assert!(batch.disable_model_invocation);
}

#[test]
fn batch_metadata_matches_upstream_skill() {
    let skills = get_bundled_skills();
    let batch = skills.iter().find(|s| s.name == "batch").unwrap();
    assert!(batch.user_invocable);
    assert_eq!(
        batch.description,
        "Research and plan a large-scale change, then execute it in parallel across 5–30 isolated worktree agents that each open a PR."
    );
    assert_eq!(
        batch.when_to_use.as_deref(),
        Some(
            "Use when the user wants to make a sweeping, mechanical change across many files (migrations, refactors, bulk renames) that can be decomposed into independent parallel units."
        )
    );
    assert_eq!(batch.argument_hint.as_deref(), Some("<instruction>"));
    assert!(
        batch
            .allowed_tools
            .as_deref()
            .is_some_and(<[String]>::is_empty)
    );
}

#[test]
fn batch_prompt_preserves_user_instruction_and_worker_contract() {
    let skills = get_bundled_skills();
    let batch = skills.iter().find(|s| s.name == "batch").unwrap();
    assert!(batch.prompt.contains("## User Instruction\n\n$ARGUMENTS"));
    assert!(batch.prompt.contains("Call the `EnterPlanMode` tool now"));
    assert!(batch.prompt.contains("use the `AskUserQuestion` tool"));
    assert!(batch.prompt.contains("Call `ExitPlanMode`"));
    assert!(batch.prompt.contains("using the `Agent` tool"));
    assert!(
        batch
            .prompt
            .contains("`isolation: \"worktree\"` and `run_in_background: true`")
    );
    assert!(
        batch
            .prompt
            .contains("Invoke the `Skill` tool with `skill: \"code-review\"")
    );
    assert!(batch.prompt.contains("to find correctness bugs"));
    assert!(batch.prompt.contains("PR: <url>"));
}

#[test]
fn simplify_prompt_matches_review_agent_flow() {
    let skills = get_bundled_skills();
    let simplify = skills.iter().find(|s| s.name == "simplify").unwrap();
    assert!(simplify.user_invocable);
    assert!(!simplify.disable_model_invocation);
    assert_eq!(
        simplify.description,
        "Review changed code for reuse, quality, and efficiency, then fix any issues found."
    );
    assert!(simplify.argument_hint.is_none());
    assert!(simplify.when_to_use.is_none());
    assert!(
        simplify
            .allowed_tools
            .as_deref()
            .is_some_and(<[String]>::is_empty)
    );
    assert!(
        simplify
            .prompt
            .contains("# Simplify: Code Review and Cleanup")
    );
    assert!(
        simplify
            .prompt
            .contains("Use the Agent tool to launch all three agents concurrently")
    );
    assert!(simplify.prompt.contains("### Agent 1: Code Reuse Review"));
    assert!(simplify.prompt.contains("### Agent 2: Code Quality Review"));
    assert!(simplify.prompt.contains("### Agent 3: Efficiency Review"));
}

#[test]
fn keybindings_not_user_invocable() {
    let skills = get_bundled_skills();
    let kb = skills
        .iter()
        .find(|s| s.name == "keybindings-help")
        .unwrap();
    assert!(!kb.user_invocable);
    assert!(kb.is_hidden);
}

#[test]
fn loop_is_always_user_invocable() {
    let skills = get_bundled_skills();
    let l = skills.iter().find(|s| s.name == "loop").unwrap();
    assert_eq!(l.gated_by, None);
    assert_eq!(l.aliases, vec!["proactive"]);
    assert_eq!(
        l.description,
        "Run a prompt or slash command on a recurring interval (e.g. /loop 5m /foo, defaults to 10m)"
    );
    assert_eq!(l.argument_hint.as_deref(), Some("[interval] <prompt>"));
    assert_eq!(
        l.allowed_tools.as_deref(),
        Some(
            [
                ToolName::CronCreate.as_str().to_string(),
                ToolName::CronDelete.as_str().to_string(),
                ToolName::ScheduleWakeup.as_str().to_string(),
                ToolName::Monitor.as_str().to_string(),
                ToolName::TaskList.as_str().to_string(),
                ToolName::TaskStop.as_str().to_string(),
                ToolName::AskUserQuestion.as_str().to_string(),
                ToolName::Skill.as_str().to_string(),
            ]
            .as_slice()
        )
    );
    assert!(l.prompt.contains("## Input\n\n$ARGUMENTS"));
    assert!(
        l.prompt
            .contains("Then immediately execute the parsed prompt now")
    );
}

#[test]
fn loop_prompt_for_empty_input_uses_project_loop_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join(".claude")).expect("mkdir");
    std::fs::write(
        dir.path().join(".claude").join("loop.md"),
        "run the checks\n",
    )
    .expect("write project loop");
    std::fs::write(dir.path().join("loop.md"), "fallback\n").expect("write cwd loop");

    let prompt =
        loop_skill::prompt_for_command("", dir.path(), dir.path(), true, false, false, false);

    assert!(prompt.contains("# /loop — schedule loop.md tasks"));
    assert!(prompt.contains("literal string `<<loop.md>>`"));
    assert!(prompt.contains("run the checks"));
    assert!(!prompt.contains("fallback"));
}

#[test]
fn loop_file_lookup_falls_back_to_live_cwd() {
    let project = tempfile::tempdir().expect("project tempdir");
    let cwd = tempfile::tempdir().expect("cwd tempdir");
    std::fs::write(cwd.path().join("loop.md"), "cwd task\n").expect("write cwd loop");

    let file = loop_skill::read_loop_file(project.path(), cwd.path()).expect("loop file");

    assert_eq!(file.content, "cwd task");
    assert_eq!(file.path, cwd.path().join("loop.md").display().to_string());
}

#[test]
fn loop_prompt_for_bare_every_uses_loop_file_interval() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join(".claude")).expect("mkdir");
    std::fs::write(dir.path().join(".claude").join("loop.md"), "daily task\n").expect("write loop");

    let prompt = loop_skill::prompt_for_command(
        "every 2 hours",
        dir.path(),
        dir.path(),
        true,
        true,
        false,
        false,
    );

    assert!(prompt.contains("interval `2h`"), "got: {prompt}");
    assert!(prompt.contains("daily task"));
}

#[test]
fn loop_sentinel_expands_to_current_loop_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join(".claude")).expect("mkdir");
    std::fs::write(dir.path().join(".claude").join("loop.md"), "tick task\n").expect("write loop");

    let prompt = loop_skill::expand_sentinel_prompt(
        loop_skill::LOOP_FILE_SENTINEL,
        dir.path(),
        dir.path(),
        false,
    )
    .expect("sentinel");

    assert!(prompt.contains("# /loop tick — tasks from "));
    assert!(prompt.contains("tick task"));
    assert!(prompt.contains("do not call ScheduleWakeup"));
}

#[test]
fn loop_sentinel_uses_short_reminder_for_unchanged_loop_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join(".claude")).expect("mkdir");
    std::fs::write(dir.path().join(".claude").join("loop.md"), "stable task\n")
        .expect("write loop");
    let mut state = loop_skill::LoopSentinelState::default();

    let first = loop_skill::expand_sentinel_prompt_with_state(
        loop_skill::LOOP_FILE_SENTINEL,
        dir.path(),
        dir.path(),
        &mut state,
        false,
    )
    .expect("first sentinel");
    let second = loop_skill::expand_sentinel_prompt_with_state(
        loop_skill::LOOP_FILE_SENTINEL,
        dir.path(),
        dir.path(),
        &mut state,
        false,
    )
    .expect("second sentinel");

    assert!(first.contains("stable task"));
    assert!(first.contains("every subsequent tick"));
    assert!(second.contains("# /loop tick — loop.md tasks"));
    assert!(second.contains("contents established earlier"));
    assert!(!second.contains("stable task"));
}

#[test]
fn loop_sentinel_resends_changed_loop_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join(".claude")).expect("mkdir");
    let path = dir.path().join(".claude").join("loop.md");
    std::fs::write(&path, "first task\n").expect("write first loop");
    let mut state = loop_skill::LoopSentinelState::default();

    let first = loop_skill::expand_sentinel_prompt_with_state(
        loop_skill::LOOP_FILE_SENTINEL,
        dir.path(),
        dir.path(),
        &mut state,
        false,
    )
    .expect("first sentinel");
    std::fs::write(&path, "second task\n").expect("write changed loop");
    let second = loop_skill::expand_sentinel_prompt_with_state(
        loop_skill::LOOP_FILE_SENTINEL,
        dir.path(),
        dir.path(),
        &mut state,
        false,
    )
    .expect("second sentinel");

    assert!(first.contains("first task"));
    assert!(second.contains("# /loop tick — tasks from "));
    assert!(second.contains("second task"));
    assert!(!second.contains("first task"));
}

#[test]
fn loop_file_dynamic_sentinel_gets_full_prompt_after_cron_sentinel() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join(".claude")).expect("mkdir");
    std::fs::write(dir.path().join(".claude").join("loop.md"), "shared task\n")
        .expect("write loop");
    let mut state = loop_skill::LoopSentinelState::default();

    let cron = loop_skill::expand_sentinel_prompt_with_state(
        loop_skill::LOOP_FILE_SENTINEL,
        dir.path(),
        dir.path(),
        &mut state,
        false,
    )
    .expect("cron sentinel");
    let dynamic = loop_skill::expand_sentinel_prompt_with_state(
        loop_skill::LOOP_FILE_DYNAMIC_SENTINEL,
        dir.path(),
        dir.path(),
        &mut state,
        false,
    )
    .expect("dynamic sentinel");

    assert!(cron.contains("# /loop tick — tasks from "));
    assert!(dynamic.contains("# /loop tick — tasks from "));
    assert!(dynamic.contains("shared task"));
    assert!(dynamic.contains("# /loop tick — loop.md tasks (dynamic pacing)"));
}

#[test]
fn loop_prompt_without_default_gate_keeps_empty_input_as_usage() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join(".claude")).expect("mkdir");
    std::fs::write(
        dir.path().join(".claude").join("loop.md"),
        "run the checks\n",
    )
    .expect("write loop");

    let prompt =
        loop_skill::prompt_for_command("", dir.path(), dir.path(), false, false, false, false);

    assert!(prompt.starts_with("Usage: /loop [interval] <prompt>"));
    assert!(!prompt.contains("run the checks"));
}

#[test]
fn loop_prompt_default_gate_without_loop_file_uses_autonomous_default() {
    let dir = tempfile::tempdir().expect("tempdir");

    let prompt =
        loop_skill::prompt_for_command("", dir.path(), dir.path(), true, false, false, false);

    assert!(prompt.contains("# /loop — schedule the autonomous default"));
    assert!(prompt.contains("literal string `<<autonomous-loop>>`"));
    assert!(prompt.contains("# Autonomous loop check"));
}

#[test]
fn loop_prompt_default_gate_accepts_compact_every_interval() {
    let dir = tempfile::tempdir().expect("tempdir");

    let prompt = loop_skill::prompt_for_command(
        "every 2h",
        dir.path(),
        dir.path(),
        true,
        true,
        false,
        false,
    );

    assert!(prompt.contains("# /loop — schedule the autonomous default"));
    assert!(prompt.contains("just the interval `2h`"));
    assert!(prompt.contains("Convert `2h` to a 5-field cron expression"));
}

#[test]
fn loop_file_truncation_respects_utf8_boundary() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join(".claude")).expect("mkdir");
    let content = format!("{}é tail", "a".repeat(24_999));
    std::fs::write(dir.path().join(".claude").join("loop.md"), content).expect("write loop");

    let prompt =
        loop_skill::prompt_for_command("", dir.path(), dir.path(), true, false, false, false);

    assert!(prompt.contains("loop.md was truncated to 25000 bytes"));
    assert!(prompt.contains("# /loop — schedule loop.md tasks"));
}

#[test]
fn loop_prompt_dynamic_default_uses_schedule_wakeup_sentinel() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join(".claude")).expect("mkdir");
    std::fs::write(dir.path().join(".claude").join("loop.md"), "dynamic task\n")
        .expect("write loop");

    let prompt =
        loop_skill::prompt_for_command("", dir.path(), dir.path(), true, true, false, false);

    assert!(prompt.contains("# /loop — loop.md tasks with dynamic pacing"));
    assert!(prompt.contains("via ScheduleWakeup — no cron"));
    assert!(prompt.contains("literal string `<<loop.md-dynamic>>`"));
    assert!(prompt.contains("arm one now with `persistent: true`"));
    assert!(prompt.contains("fallback heartbeat (lean 1200–1800s)"));
    assert!(prompt.contains("If woken by a `<task-notification>`"));
    assert!(prompt.contains("dynamic task"));
}

#[test]
fn loop_prompt_dynamic_non_interval_self_paces() {
    let dir = tempfile::tempdir().expect("tempdir");

    let prompt = loop_skill::prompt_for_command(
        "check CI",
        dir.path(),
        dir.path(),
        false,
        true,
        false,
        false,
    );

    assert!(prompt.contains("# /loop — schedule a recurring or self-paced prompt"));
    assert!(prompt.contains("## Dynamic mode"));
    assert!(prompt.contains("call ScheduleWakeup"));
    assert!(prompt.contains("arm one now with `persistent: true`"));
    assert!(prompt.contains("idle ticks past the 5-minute cache window are pure overhead"));
    assert!(prompt.contains("If you were woken by a `<task-notification>`"));
    assert!(prompt.contains("## Input\n\ncheck CI"));
}

#[test]
fn loop_prompt_dynamic_keeps_cron_table_for_fixed_interval_mode() {
    let dir = tempfile::tempdir().expect("tempdir");

    let prompt = loop_skill::prompt_for_command(
        "5m /status",
        dir.path(),
        dir.path(),
        false,
        true,
        false,
        false,
    );

    assert!(prompt.contains("## Fixed-interval mode (rules 1 and 2)"));
    assert!(prompt.contains("| Interval pattern"));
    assert!(prompt.contains("`Nm` where N <= 59"));
    assert!(prompt.contains("`90m` -> 1.5h which cron can't express"));
    assert!(prompt.contains("## Dynamic mode"));
}

#[test]
fn autonomous_loop_sentinel_expands_to_tick_prompt() {
    let dir = tempfile::tempdir().expect("tempdir");

    let prompt = loop_skill::expand_sentinel_prompt(
        loop_skill::AUTONOMOUS_LOOP_SENTINEL,
        dir.path(),
        dir.path(),
        false,
    )
    .expect("sentinel");

    assert!(prompt.contains("# Autonomous loop check"));
    assert!(prompt.contains("# Autonomous loop tick"));
    assert!(prompt.contains("do not call ScheduleWakeup"));
}

#[test]
fn autonomous_loop_sentinel_uses_short_reminder_after_first_tick() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut state = loop_skill::LoopSentinelState::default();

    let first = loop_skill::expand_sentinel_prompt_with_state(
        loop_skill::AUTONOMOUS_LOOP_SENTINEL,
        dir.path(),
        dir.path(),
        &mut state,
        false,
    )
    .expect("first sentinel");
    let second = loop_skill::expand_sentinel_prompt_with_state(
        loop_skill::AUTONOMOUS_LOOP_SENTINEL,
        dir.path(),
        dir.path(),
        &mut state,
        false,
    )
    .expect("second sentinel");

    assert!(first.contains("# Autonomous loop check"));
    assert!(second.starts_with("# Autonomous loop tick"));
    assert!(!second.contains("You're being invoked on a timer"));
}

#[test]
fn autonomous_dynamic_sentinel_gets_full_prompt_after_cron_sentinel() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut state = loop_skill::LoopSentinelState::default();

    let cron = loop_skill::expand_sentinel_prompt_with_state(
        loop_skill::AUTONOMOUS_LOOP_SENTINEL,
        dir.path(),
        dir.path(),
        &mut state,
        false,
    )
    .expect("cron sentinel");
    let dynamic = loop_skill::expand_sentinel_prompt_with_state(
        loop_skill::AUTONOMOUS_LOOP_DYNAMIC_SENTINEL,
        dir.path(),
        dir.path(),
        &mut state,
        false,
    )
    .expect("dynamic sentinel");

    assert!(cron.contains("# Autonomous loop check"));
    assert!(dynamic.contains("# Autonomous loop check"));
    assert!(dynamic.contains("# Autonomous loop tick (dynamic pacing)"));
}

#[test]
fn dynamic_loop_sentinel_expands_to_dynamic_tick_prompt() {
    let dir = tempfile::tempdir().expect("tempdir");

    let prompt = loop_skill::expand_sentinel_prompt(
        loop_skill::AUTONOMOUS_LOOP_DYNAMIC_SENTINEL,
        dir.path(),
        dir.path(),
        false,
    )
    .expect("sentinel");

    assert!(prompt.contains("# Autonomous loop check"));
    assert!(prompt.contains("# Autonomous loop tick (dynamic pacing)"));
    assert!(prompt.contains("literal sentinel `<<autonomous-loop-dynamic>>`"));
    assert!(prompt.contains("1200–1800s fallback heartbeat"));
}

#[test]
fn loop_file_content_is_truncated() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join(".claude")).expect("mkdir");
    let content = format!("{}\n{}", "a".repeat(24_900), "b".repeat(500));
    std::fs::write(dir.path().join(".claude").join("loop.md"), content).expect("write loop");

    let file = loop_skill::read_loop_file(dir.path(), dir.path()).expect("loop file");

    assert!(file.content.contains("WARNING: loop.md was truncated"));
    assert!(!file.content.contains(&"b".repeat(500)));
}

#[test]
fn empty_loop_file_is_ignored() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join(".claude")).expect("mkdir");
    std::fs::write(dir.path().join(".claude").join("loop.md"), " \n\t\n").expect("write loop");

    assert!(loop_skill::read_loop_file(dir.path(), dir.path()).is_none());

    let prompt =
        loop_skill::prompt_for_command("", dir.path(), dir.path(), true, false, false, false);
    assert!(prompt.contains("# /loop — schedule the autonomous default"));
}

#[test]
fn schedule_is_gated_by_agent_triggers_remote() {
    let skills = get_bundled_skills();
    let s = skills.iter().find(|s| s.name == "schedule").unwrap();
    assert_eq!(s.gated_by, Some(Feature::AgentTriggersRemote));
}

#[test]
fn claude_api_is_gated_by_building_claude_apps() {
    let skills = get_bundled_skills();
    let c = skills.iter().find(|s| s.name == "claude-api").unwrap();
    assert_eq!(c.gated_by, Some(Feature::BuildingClaudeApps));
}

#[test]
fn dream_hunter_chrome_runskillgen_present_and_gated() {
    let skills = get_bundled_skills();
    let dream = skills.iter().find(|s| s.name == "dream").unwrap();
    let hunter = skills.iter().find(|s| s.name == "hunter").unwrap();
    let chrome = skills
        .iter()
        .find(|s| s.name == "claude-in-chrome")
        .unwrap();
    let rsg = skills
        .iter()
        .find(|s| s.name == "run-skill-generator")
        .unwrap();
    assert_eq!(dream.gated_by, Some(Feature::KairosDream));
    assert_eq!(hunter.gated_by, Some(Feature::ReviewArtifact));
    assert_eq!(chrome.gated_by, Some(Feature::ClaudeInChrome));
    assert_eq!(rsg.gated_by, Some(Feature::RunSkillGenerator));
}

#[test]
fn visible_filters_by_features() {
    let manager = crate::SkillManager::new();
    register_bundled(&manager);

    let no_features = Features::empty();
    let visible_empty_skills = manager.visible(&no_features);
    let visible_empty: Vec<&str> = visible_empty_skills
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    // Even with no features enabled, ungated skills appear — including the
    // formerly-ant general-purpose skills.
    assert!(visible_empty.contains(&"update-config"));
    assert!(visible_empty.contains(&"verify"));
    // /loop is always registered; dynamic/default-prompt sub-modes are
    // configured separately.
    assert!(visible_empty.contains(&"loop"));
    // Feature-gated skills should NOT appear.
    assert!(!visible_empty.contains(&"dream"));

    let visible_default_skills = manager.visible(&Features::with_defaults());
    let visible_default: Vec<&str> = visible_default_skills
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert!(
        visible_default.contains(&"loop"),
        "default features should expose /loop"
    );

    let mut features = Features::empty();
    features.enable(Feature::KairosDream);
    let visible_some_skills = manager.visible(&features);
    let visible_some: Vec<&str> = visible_some_skills
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert!(visible_some.contains(&"loop"));
    assert!(visible_some.contains(&"dream"));
    assert!(!visible_some.contains(&"hunter")); // not enabled
}

#[test]
fn all_bundled_are_bundled_source() {
    let skills = get_bundled_skills();
    for skill in &skills {
        assert!(
            matches!(skill.source, crate::SkillSource::Bundled),
            "skill {} should be Bundled source",
            skill.name
        );
    }
}
