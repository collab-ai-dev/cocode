use super::*;
use crate::types::TeamMember;

fn make_test_team_file() -> TeamFile {
    TeamFile {
        name: "test-team".to_string(),
        description: Some("A test team".to_string()),
        created_at: 1000,
        lead_agent_id: "leader@test-team".to_string(),
        lead_session_id: Some("session-1".to_string()),
        hidden_pane_ids: Vec::new(),
        team_allowed_paths: Vec::new(),
        members: vec![TeamMember {
            agent_id: "worker-1@test-team".to_string(),
            name: "worker-1".to_string(),
            agent_type: None,
            model: None,
            prompt: Some("Do research".to_string()),
            color: Some("blue".to_string()),
            plan_mode_required: false,
            joined_at: 1000,
            tmux_pane_id: String::new(),
            cwd: "/tmp".to_string(),
            worktree_path: None,
            session_id: None,
            subscriptions: Vec::new(),
            backend_type: None,
            is_active: true,
            mode: None,
        }],
    }
}

#[test]
fn test_team_dir_path() {
    let dir = get_team_dir("My Team");
    let path_str = dir.to_string_lossy();
    assert!(path_str.contains("teams"));
    assert!(path_str.contains("my-team"));
}

#[test]
fn test_team_file_path() {
    let path = get_team_file_path("test");
    assert!(path.to_string_lossy().ends_with("config.json"));
}

#[test]
fn test_write_and_read_team_file() {
    let dir = tempfile::tempdir().unwrap();
    let team_dir = dir.path().join("test-team");
    std::fs::create_dir_all(&team_dir).unwrap();

    let tf = make_test_team_file();
    let path = team_dir.join("config.json");
    let content = serde_json::to_string_pretty(&tf).unwrap();
    std::fs::write(&path, &content).unwrap();

    let parsed: TeamFile = serde_json::from_str(&content).unwrap();
    assert_eq!(parsed.name, "test-team");
    assert_eq!(parsed.members.len(), 1);
    assert_eq!(parsed.members[0].name, "worker-1");
    assert_eq!(parsed.members[0].prompt.as_deref(), Some("Do research"));
    assert!(parsed.members[0].is_active);
}

#[test]
fn test_team_file_serde_new_fields() {
    let tf = TeamFile {
        name: "test".to_string(),
        description: None,
        created_at: 1000,
        lead_agent_id: "lead@test".to_string(),
        lead_session_id: None,
        hidden_pane_ids: vec!["pane-1".to_string()],
        team_allowed_paths: vec![crate::types::TeamAllowedPath {
            path: "/src".to_string(),
            tool_name: "Edit".to_string(),
            added_by: "leader".to_string(),
            added_at: 2000,
        }],
        members: Vec::new(),
    };

    let json = serde_json::to_string(&tf).unwrap();
    assert!(json.contains("hiddenPaneIds"));
    assert!(json.contains("teamAllowedPaths"));
    assert!(json.contains("pane-1"));

    let parsed: TeamFile = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.hidden_pane_ids, vec!["pane-1"]);
    assert_eq!(parsed.team_allowed_paths.len(), 1);
    assert_eq!(parsed.team_allowed_paths[0].tool_name, "Edit");
}

#[test]
fn test_list_team_names_empty() {
    // Won't find any teams in a temp dir, but validates no panic
    let names = list_team_names();
    // May or may not be empty depending on whether ~/.coco/teams/ exists
    let _ = names;
}

#[test]
fn test_sanitize_name_for_dir() {
    assert_eq!(crate::types::sanitize_name("My Team!"), "my-team-");
    assert_eq!(crate::types::sanitize_name("test-1"), "test-1");
}

#[tokio::test]
async fn cleanup_destroys_member_worktrees_and_tasks_dir() {
    // Isolate the teams root: this test captures `get_team_dir(team)` and later
    // asserts `cleanup_team_directories(team)` removed it, so a concurrent test
    // flipping `COCO_TEAMS_DIR` mid-body (threaded `cargo test`) would point the
    // cleanup at a different base than the captured path. Shares the crate-wide
    // `ENV_LOCK` (see `crate::test_support`).
    let _teams = crate::test_support::isolate_teams_dir().await;
    let team_name = format!("agentteam-cleanup-{}", uuid::Uuid::new_v4().simple());

    // Stand-in worktree: a plain dir. `destroy_worktree`'s
    // `git worktree remove` fails on a non-worktree and falls back to
    // remove_dir_all, so the dir should still be gone after cleanup.
    let worktree = std::env::temp_dir().join(format!("wt-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&worktree).unwrap();
    std::fs::write(worktree.join("f.txt"), "x").unwrap();

    // Team file with a member carrying that worktree path.
    let team_dir = get_team_dir(&team_name);
    std::fs::create_dir_all(&team_dir).unwrap();
    let mut tf = make_test_team_file();
    tf.name = team_name.clone();
    tf.members[0].worktree_path = Some(worktree.to_string_lossy().to_string());
    std::fs::write(
        get_team_file_path(&team_name),
        serde_json::to_string_pretty(&tf).unwrap(),
    )
    .unwrap();

    // The team's task-list dir under the same root `TaskList::open` uses.
    let task_list_id = crate::types::sanitize_name(&team_name);
    let tasks_dir = coco_config::global_config::config_home()
        .join("tasks")
        .join(coco_tasks::task_list::sanitize_path_component(
            &task_list_id,
        ));
    std::fs::create_dir_all(&tasks_dir).unwrap();
    std::fs::write(tasks_dir.join("t.json"), "{}").unwrap();

    let outcome = cleanup_team_directories(&team_name).expect("cleanup ok");

    assert!(!worktree.exists(), "member worktree should be destroyed");
    assert!(!team_dir.exists(), "team dir should be removed");
    assert!(!tasks_dir.exists(), "task-list dir should be removed");
    assert!(
        outcome.tasks_dir_removed,
        "tasks_dir_removed must be true on the success path (gates notify)",
    );
}
