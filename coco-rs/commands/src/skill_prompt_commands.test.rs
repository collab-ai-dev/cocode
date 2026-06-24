use std::path::Path;
use std::path::PathBuf;

use coco_skills::SkillManager;
use coco_skills::bundled::register_bundled;
use coco_types::CommandType;
use coco_types::Features;
use coco_types::UserType;

use super::*;

#[tokio::test]
async fn batch_command_renders_instruction_inside_upstream_prompt() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_git_repo(dir.path());
    let reg = registry_for_root(dir.path().to_path_buf());

    let cmd = reg.get("batch").expect("batch command");
    assert_eq!(cmd.base.argument_hint.as_deref(), Some("<instruction>"));
    assert_eq!(
        cmd.base.description,
        "Research and plan a large-scale change, then execute it in parallel across 5–30 isolated worktree agents that each open a PR."
    );
    match &cmd.command_type {
        CommandType::Prompt(data) => {
            assert!(
                data.allowed_tools
                    .as_deref()
                    .is_some_and(<[String]>::is_empty)
            );
        }
        other => panic!("unexpected command type: {other:?}"),
    }

    let text = prompt_text(
        reg.execute_command("batch", "migrate auth callers")
            .await
            .expect("execute batch"),
    );
    assert!(text.contains("## User Instruction\n\nmigrate auth callers\n\n## Phase 1"));
    assert!(text.contains("`isolation: \"worktree\"` and `run_in_background: true`"));
    assert!(text.contains("Invoke the `Skill` tool with `skill: \"code-review\""));
    assert!(text.contains("to find correctness bugs"));
    assert!(!text.contains("ARGUMENTS: migrate auth callers"));
}

#[tokio::test]
async fn batch_command_missing_instruction_matches_upstream_prompt_guard() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_git_repo(dir.path());
    let reg = registry_for_root(dir.path().to_path_buf());

    let text = prompt_text(
        reg.execute_command("batch", "   ")
            .await
            .expect("execute batch"),
    );
    assert_eq!(text, BATCH_MISSING_INSTRUCTION_MESSAGE);
}

#[tokio::test]
async fn batch_command_requires_git_repo_for_worktree_agents() {
    let dir = tempfile::tempdir().expect("tempdir");
    let reg = registry_for_root(dir.path().to_path_buf());

    let text = prompt_text(
        reg.execute_command("batch", "migrate auth")
            .await
            .expect("execute batch"),
    );
    assert_eq!(text, BATCH_NOT_A_GIT_REPO_MESSAGE);
}

#[tokio::test]
async fn simplify_command_matches_upstream_prompt_contract() {
    let reg = registry_for_root(PathBuf::from("."));

    let cmd = reg.get("simplify").expect("simplify command");
    assert!(cmd.base.argument_hint.is_none());
    assert_eq!(
        cmd.base.description,
        "Review changed code for reuse, quality, and efficiency, then fix any issues found."
    );
    match &cmd.command_type {
        CommandType::Prompt(data) => {
            assert!(
                data.allowed_tools
                    .as_deref()
                    .is_some_and(<[String]>::is_empty)
            );
        }
        other => panic!("unexpected command type: {other:?}"),
    }

    let text = prompt_text(
        reg.execute_command("simplify", "focus on command permissions")
            .await
            .expect("execute simplify"),
    );
    assert!(text.contains("# Simplify: Code Review and Cleanup"));
    assert!(text.contains("Use the Agent tool to launch all three agents concurrently"));
    assert!(text.contains("### Agent 1: Code Reuse Review"));
    assert!(text.contains("### Agent 2: Code Quality Review"));
    assert!(text.contains("### Agent 3: Efficiency Review"));
    assert!(text.contains("## Additional Focus\n\nfocus on command permissions"));
    assert!(!text.contains("ARGUMENTS: focus on command permissions"));
    assert!(!text.contains("worktree"));
}

#[tokio::test]
async fn loop_command_renders_input_inside_fixed_interval_prompt() {
    let reg = registry_for_root(PathBuf::from("."));

    let cmd = reg.get("loop").expect("loop command");
    assert_eq!(
        reg.get("proactive").map(|c| c.base.name.as_str()),
        Some("loop")
    );
    assert_eq!(
        cmd.base.argument_hint.as_deref(),
        Some("[interval] <prompt>")
    );
    assert_eq!(
        cmd.base.description,
        "Run a prompt or slash command on a recurring interval (e.g. /loop 5m /foo, defaults to 10m)"
    );
    assert_eq!(
        cmd.base.when_to_use.as_deref(),
        Some(
            "When the user wants to set up a recurring task, poll for status, or run something repeatedly on an interval (e.g. \"check the deploy every 5 minutes\", \"keep running /babysit-prs\"). Do NOT invoke for one-off tasks."
        )
    );

    let text = prompt_text(
        reg.execute_command("loop", "5m /status")
            .await
            .expect("execute loop"),
    );
    assert!(text.contains("# /loop — schedule a recurring prompt"));
    assert!(text.contains("Call CronCreate"));
    assert!(text.contains("CronDelete"));
    assert!(text.contains("## Input\n\n5m /status"));
    assert!(!text.contains("## Offer cloud first"));
    assert!(!text.contains("ARGUMENTS: 5m /status"));
}

#[tokio::test]
async fn loop_command_offers_cloud_schedule_when_remote_triggers_enabled() {
    let mut features = Features::with_defaults();
    features.enable(coco_types::Feature::AgentTriggersRemote);
    let reg = registry_for_root_with_features(PathBuf::from("."), features);

    let text = prompt_text(
        reg.execute_command("loop", "2h summarize PR status")
            .await
            .expect("execute loop"),
    );

    assert!(text.contains("## Offer cloud first"));
    assert!(text.contains("call AskUserQuestion first"));
    assert!(text.contains("Cloud schedule (recommended)"));
    assert!(text.contains("Invoke the `schedule` skill directly via the Skill tool"));
    assert!(text.contains(
        "_Runs until you close this session · For durable cloud-based loops, use /schedule_"
    ));
}

#[tokio::test]
async fn loop_command_dynamic_fixed_interval_offers_cloud_schedule_when_remote_triggers_enabled() {
    let mut features = Features::with_defaults();
    features.enable(coco_types::Feature::AgentTriggersRemote);
    let reg = registry_for_root_with_features_and_loop_config(
        PathBuf::from("."),
        features,
        coco_config::LoopConfig {
            dynamic_enabled: true,
            ..Default::default()
        },
    );

    let text = prompt_text(
        reg.execute_command("loop", "daily summarize PR status")
            .await
            .expect("execute loop"),
    );

    assert!(text.contains("# /loop — schedule a recurring or self-paced prompt"));
    assert!(text.contains("## Offer cloud first"));
    assert!(text.contains("daily-cadence loop won't fire before this session closes"));
    assert!(text.contains("## Dynamic mode"));
}

#[tokio::test]
async fn loop_command_metadata_uses_independent_mode_toggles() {
    let dynamic_only = registry_for_root_with_loop_config(
        PathBuf::from("."),
        coco_config::LoopConfig {
            dynamic_enabled: true,
            ..Default::default()
        },
    );
    let cmd = dynamic_only.get("loop").expect("loop command");
    assert_eq!(
        cmd.base.argument_hint.as_deref(),
        Some("[interval] <prompt>")
    );
    assert_eq!(
        cmd.base.description,
        "Run a prompt or slash command on a recurring interval (e.g. /loop 5m /foo). Omit the interval to let the model self-pace."
    );

    let default_only = registry_for_root_with_loop_config(
        PathBuf::from("."),
        coco_config::LoopConfig {
            default_prompt_enabled: true,
            ..Default::default()
        },
    );
    let cmd = default_only.get("loop").expect("loop command");
    assert_eq!(
        cmd.base.argument_hint.as_deref(),
        Some("[interval] [prompt]")
    );
    assert_eq!(
        cmd.base.description,
        "Run a prompt or slash command on a recurring interval (e.g. /loop 5m /foo, defaults to 10m)"
    );
}

#[tokio::test]
async fn loop_command_empty_input_uses_loop_md_default_when_present() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join(".claude")).expect("mkdir");
    std::fs::write(
        dir.path().join(".claude").join("loop.md"),
        "- Check CI\n- Summarize blockers\n",
    )
    .expect("write loop.md");

    let reg = registry_for_root_with_loop_config(
        dir.path().to_path_buf(),
        coco_config::LoopConfig {
            default_prompt_enabled: true,
            dynamic_enabled: true,
            ..Default::default()
        },
    );

    let text = prompt_text(reg.execute_command("loop", "").await.expect("execute loop"));
    assert!(text.contains("# /loop — loop.md tasks with dynamic pacing"));
    assert!(text.contains("via ScheduleWakeup — no cron"));
    assert!(text.contains("literal string `<<loop.md-dynamic>>`"));
    assert!(text.contains("## Loop tasks (from "));
    assert!(text.contains("- Check CI\n- Summarize blockers"));
}

#[tokio::test]
async fn loop_command_empty_input_without_default_prompt_shows_usage() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join(".claude")).expect("mkdir");
    std::fs::write(dir.path().join(".claude").join("loop.md"), "- Check CI\n")
        .expect("write loop.md");

    let reg = registry_for_root(dir.path().to_path_buf());

    let text = prompt_text(reg.execute_command("loop", "").await.expect("execute loop"));
    assert!(text.starts_with("Usage: /loop [interval] <prompt>"));
    assert!(!text.contains("- Check CI"));
}

#[tokio::test]
async fn loop_command_empty_input_with_loop_gates_uses_autonomous_default() {
    let dir = tempfile::tempdir().expect("tempdir");

    let reg = registry_for_root_with_loop_config(
        dir.path().to_path_buf(),
        coco_config::LoopConfig {
            default_prompt_enabled: true,
            dynamic_enabled: true,
            ..Default::default()
        },
    );

    let text = prompt_text(reg.execute_command("loop", "").await.expect("execute loop"));
    assert!(text.contains("# /loop — autonomous default with dynamic pacing"));
    assert!(text.contains("literal string `<<autonomous-loop-dynamic>>`"));
    assert!(text.contains("# Autonomous loop check"));
}

fn registry_for_root(project_root: PathBuf) -> CommandRegistry {
    registry_for_root_with_features(project_root, Features::with_defaults())
}

fn registry_for_root_with_features(project_root: PathBuf, features: Features) -> CommandRegistry {
    registry_for_root_with_features_and_loop_config(
        project_root,
        features,
        coco_config::LoopConfig::default(),
    )
}

fn registry_for_root_with_loop_config(
    project_root: PathBuf,
    loop_config: coco_config::LoopConfig,
) -> CommandRegistry {
    registry_for_root_with_features_and_loop_config(
        project_root,
        Features::with_defaults(),
        loop_config,
    )
}

fn registry_for_root_with_features_and_loop_config(
    project_root: PathBuf,
    features: Features,
    loop_config: coco_config::LoopConfig,
) -> CommandRegistry {
    let sm = SkillManager::new();
    register_bundled(&sm);
    build_command_registry(
        &sm,
        &[],
        UserType::Human,
        features,
        loop_config,
        project_root,
        PathBuf::from("/home/test"),
        None,
        &coco_config::SkillOverrideTiers::default(),
    )
}

fn init_git_repo(path: &Path) {
    let out = std::process::Command::new("git")
        .arg("init")
        .arg(path)
        .output()
        .expect("run git init");
    assert!(
        out.status.success(),
        "git init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn prompt_text(result: CommandResult) -> String {
    match result {
        CommandResult::Prompt { parts, .. } => parts
            .into_iter()
            .filter_map(|part| match part {
                PromptPart::Text { text } => Some(text),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(""),
        other => panic!("expected Prompt, got {other:?}"),
    }
}
