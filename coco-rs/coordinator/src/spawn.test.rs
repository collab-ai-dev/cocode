use super::*;

#[test]
fn test_get_teammate_command() {
    let cmd = get_teammate_command();
    // Should return something (either env var or current exe or "claude")
    assert!(!cmd.is_empty());
}

#[test]
fn test_build_teammate_command_basic() {
    let config = TeammateSpawnConfig {
        name: "researcher".into(),
        team_name: "my-team".into(),
        color: Some(crate::constants::AgentColorName::Blue),
        plan_mode_required: false,
        prompt: "Find the bug".into(),
        cwd: "/project".into(),
        model: Some("anthropic/claude-opus-4-7".into()),
        system_prompt: None,
        system_prompt_mode: crate::pane::SystemPromptMode::Default,
        worktree_path: None,
        parent_session_id: "session-1".into(),
        permissions: Vec::new(),
        allow_permission_prompts: false,
        ..Default::default()
    };

    let cmd = build_teammate_command(&config);
    assert!(cmd.contains("cd /project &&"));
    assert!(cmd.contains("--models.main=anthropic/claude-opus-4-7"));
    // Identity rides COCO_* env, NOT CLI flags (clap defines none of the
    // identity flags and would reject them on the child's launch).
    assert!(!cmd.contains("--agent-id"));
    assert!(!cmd.contains("--agent-name"));
    assert!(!cmd.contains("--team-name"));
    assert!(!cmd.contains("--agent-color"));
    assert!(!cmd.contains("--parent-session-id"));
    assert!(cmd.contains("COCO_AGENT_ID=researcher@my-team"));
    assert!(cmd.contains("COCO_AGENT_NAME=researcher"));
    assert!(cmd.contains("COCO_TEAM_NAME=my-team"));
    assert!(cmd.contains("COCO_AGENT_COLOR=blue"));
    assert!(!cmd.contains("--plan-mode-required"));
}

#[test]
fn test_build_teammate_command_plan_mode() {
    let config = TeammateSpawnConfig {
        name: "coder".into(),
        team_name: "team".into(),
        color: None,
        plan_mode_required: true,
        prompt: "Fix it".into(),
        cwd: "/tmp".into(),
        model: None,
        system_prompt: None,
        system_prompt_mode: crate::pane::SystemPromptMode::Default,
        worktree_path: None,
        parent_session_id: "sess".into(),
        permissions: Vec::new(),
        allow_permission_prompts: false,
        ..Default::default()
    };

    let cmd = build_teammate_command(&config);
    // plan-mode rides env now (clap has no --plan-mode-required flag).
    assert!(!cmd.contains("--plan-mode-required"));
    assert!(cmd.contains("COCO_PLAN_MODE_REQUIRED=1"));
    assert!(!cmd.contains("--agent-color"));
}

#[test]
fn test_build_inherited_env_vars() {
    let config = TeammateSpawnConfig {
        name: "worker".into(),
        team_name: "t".into(),
        color: Some(crate::constants::AgentColorName::Red),
        plan_mode_required: true,
        prompt: String::new(),
        cwd: String::new(),
        model: None,
        system_prompt: None,
        system_prompt_mode: crate::pane::SystemPromptMode::Default,
        worktree_path: None,
        parent_session_id: "s".into(),
        permissions: Vec::new(),
        allow_permission_prompts: false,
        ..Default::default()
    };

    let env = build_inherited_env_vars(&config);
    // Feature::AgentTeams gate is propagated via the COCO_FEATURE_*
    // namespace so spawned children re-resolve it the same way the
    // parent did.
    assert!(env.contains("COCO_FEATURE_AGENT_TEAMS=1"));
    assert!(env.contains("COCO_AGENT_COLOR=red"));
    assert!(env.contains("COCO_PLAN_MODE_REQUIRED=1"));
    // Worker identity must be in env so the child can read its own identity
    // when callers go through `crate::identity::*` helpers.
    assert!(env.contains("COCO_AGENT_ID=worker@t"));
    assert!(env.contains("COCO_AGENT_NAME=worker"));
    assert!(env.contains("COCO_TEAM_NAME=t"));
}
