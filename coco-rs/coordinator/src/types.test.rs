use super::*;

// ── TeammateIdentity ──

#[test]
fn test_teammate_identity_serde_roundtrip() {
    let identity = TeammateIdentity {
        agent_id: "researcher@my-team".into(),
        agent_name: "researcher".into(),
        team_name: "my-team".into(),
        color: Some(AgentColorName::Blue),
        plan_mode_required: false,
    };
    let json = serde_json::to_string(&identity).expect("serialize");
    let parsed: TeammateIdentity = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.agent_id, "researcher@my-team");
    assert_eq!(parsed.color, Some(AgentColorName::Blue));
}

// ── AgentSpawnResult ──

#[test]
fn test_agent_spawn_result_success() {
    let result = AgentSpawnResult::success("id-1".into(), "worker-1".into());
    assert_eq!(result.status, SubAgentStatus::Running);
    assert!(result.error.is_none());
}

#[test]
fn test_agent_spawn_result_failure() {
    let result = AgentSpawnResult::failure("id-2".into(), "worker-2".into(), "timeout".into());
    assert_eq!(result.status, SubAgentStatus::Failed);
    assert_eq!(result.error.as_deref(), Some("timeout"));
}

// ── TeamManager ──

#[tokio::test]
async fn test_team_manager_register_and_remove() {
    let team_file = TeamFile {
        name: "test-team".into(),
        description: None,
        created_at: 1000,
        lead_agent_id: "lead-1".into(),
        lead_session_id: None,
        hidden_pane_ids: Vec::new(),
        team_allowed_paths: Vec::new(),
        members: vec![TeamMember {
            agent_id: "worker-1".into(),
            name: "worker".into(),
            agent_type: Some("researcher".into()),
            model: None,
            prompt: None,
            color: None,
            plan_mode_required: false,
            joined_at: 1000,
            tmux_pane_id: String::new(),
            cwd: "/project".into(),
            worktree_path: None,
            session_id: None,
            subscriptions: Vec::new(),
            backend_type: None,
            is_active: true,
            mode: None,
        }],
    };

    let manager = TeamManager::new("test-team".into(), team_file);

    // Register a running agent
    manager
        .register_agent(SubAgentState {
            agent_id: "worker-1".into(),
            name: "worker".into(),
            status: SubAgentStatus::Running,
            turns: 0,
            model: None,
            working_dir: Some("/project".into()),
            last_message: None,
        })
        .await;

    let running = manager.running_agents().await;
    assert_eq!(running.len(), 1);

    // Remove the member
    assert!(manager.remove_member("worker-1").await);
    assert_eq!(manager.member_count().await, 0);
}

#[tokio::test]
async fn test_team_manager_mailbox() {
    let team_file = TeamFile {
        name: "mail-test".into(),
        description: None,
        created_at: 1000,
        lead_agent_id: "lead-1".into(),
        lead_session_id: None,
        hidden_pane_ids: Vec::new(),
        team_allowed_paths: Vec::new(),
        members: vec![],
    };

    let manager = TeamManager::new("mail-test".into(), team_file);

    // Empty mailbox
    let msgs = manager.read_mailbox("alice").await;
    assert!(msgs.is_empty());

    // Send a message
    let msg = AgentMessage {
        from_agent: "bob".into(),
        to_agent: "alice".into(),
        content: AgentMessageContent::Text {
            text: "hello".into(),
        },
        timestamp: 1000,
    };
    manager.send_message("alice", msg).await;

    // Read drains the mailbox
    let msgs = manager.read_mailbox("alice").await;
    assert_eq!(msgs.len(), 1);
    let msgs = manager.read_mailbox("alice").await;
    assert!(msgs.is_empty());
}

#[tokio::test]
async fn test_team_manager_is_leader() {
    let team_file = TeamFile {
        name: "leader-test".into(),
        description: None,
        created_at: 1000,
        lead_agent_id: "lead-xyz".into(),
        lead_session_id: None,
        hidden_pane_ids: Vec::new(),
        team_allowed_paths: Vec::new(),
        members: vec![],
    };
    let manager = TeamManager::new("leader-test".into(), team_file);
    assert!(manager.is_leader("lead-xyz").await);
    assert!(!manager.is_leader("worker-1").await);
}

// ── Utility functions ──

#[test]
fn test_sanitize_name() {
    assert_eq!(sanitize_name("My Team!"), "my-team-");
    assert_eq!(sanitize_name("test123"), "test123");
    assert_eq!(sanitize_name("a b.c"), "a-b-c");
}

#[test]
fn test_sanitize_agent_name() {
    assert_eq!(sanitize_agent_name("agent@team"), "agent-team");
    assert_eq!(sanitize_agent_name("no-at-sign"), "no-at-sign");
}
