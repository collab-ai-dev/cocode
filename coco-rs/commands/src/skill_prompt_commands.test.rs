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
    assert!(text.contains("Invoke the `Skill` tool with `skill: \"simplify\""));
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

fn registry_for_root(project_root: PathBuf) -> CommandRegistry {
    let sm = SkillManager::new();
    register_bundled(&sm);
    build_command_registry(
        &sm,
        &[],
        UserType::Human,
        Features::with_defaults(),
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
