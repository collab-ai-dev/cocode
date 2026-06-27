use std::sync::Arc;

use pretty_assertions::assert_eq;
use tokio::sync::RwLock;

use super::*;
use crate::session_team::session_team_name;

fn bootstrap_request(
    team_name: &str,
    session_id: &str,
) -> coco_tool_runtime::InitializeSessionTeamRequest {
    coco_tool_runtime::InitializeSessionTeamRequest {
        team_name: team_name.to_string(),
        leader_session_id: session_id.to_string(),
        leader_agent_type: None,
        leader_model: Some("claude-test".to_string()),
        cwd: std::path::PathBuf::from("/tmp/project"),
        task_list_router: None,
    }
}

#[tokio::test]
async fn initialize_session_team_writes_leader_only_roster() {
    let _teams = crate::test_support::isolate_teams_dir().await;
    let store = TeamRosterStore::new(Arc::new(RwLock::new(None)));

    let session_id = "abcdef1234567890";
    let team_name = session_team_name(session_id);
    assert_eq!(team_name, "session-abcdef12");

    let result = store
        .initialize_session_team(bootstrap_request(&team_name, session_id))
        .await
        .expect("bootstrap ok");
    assert!(matches!(
        result,
        InitializeSessionTeamResult::Created { .. }
    ));

    // The team became active under the deterministic name.
    assert_eq!(
        store.active_team_name().await.as_deref(),
        Some(team_name.as_str())
    );

    // The on-disk roster is leader-only and matches the CC shape.
    let team_file = team_file::read_team_file(&team_name)
        .expect("read ok")
        .expect("team file written");
    assert_eq!(team_file.name, team_name);
    assert_eq!(team_file.lead_session_id.as_deref(), Some(session_id));
    assert_eq!(team_file.members.len(), 1);
    let lead = &team_file.members[0];
    assert_eq!(lead.name, TEAM_LEAD_NAME);
    assert_eq!(lead.backend_type, Some(BackendType::InProcess));
    assert!(lead.is_active);
    assert_eq!(lead.model.as_deref(), Some("claude-test"));
    assert_eq!(lead.tmux_pane_id, "");
}

#[tokio::test]
async fn initialize_session_team_is_idempotent_when_already_active() {
    let _teams = crate::test_support::isolate_teams_dir().await;
    let store = TeamRosterStore::new(Arc::new(RwLock::new(None)));

    let session_id = "fedcba9876543210";
    let team_name = session_team_name(session_id);

    store
        .initialize_session_team(bootstrap_request(&team_name, session_id))
        .await
        .expect("first bootstrap ok");
    // Second call sees an already-active team and no-ops.
    let second = store
        .initialize_session_team(bootstrap_request(&team_name, session_id))
        .await
        .expect("second bootstrap ok");
    assert!(matches!(
        second,
        InitializeSessionTeamResult::AlreadyActive { .. }
    ));
}

#[tokio::test]
async fn initialize_session_team_reuses_existing_on_disk_without_clobber() {
    let _teams = crate::test_support::isolate_teams_dir().await;
    let session_id = "0011223344556677";
    let team_name = session_team_name(session_id);

    // First store writes the roster, then we drop its active_team so the
    // second store starts cold but the on-disk file remains (resumed session).
    let store1 = TeamRosterStore::new(Arc::new(RwLock::new(None)));
    store1
        .initialize_session_team(bootstrap_request(&team_name, session_id))
        .await
        .expect("first bootstrap ok");
    let original = team_file::read_team_file(&team_name)
        .expect("read ok")
        .expect("written");

    let store2 = TeamRosterStore::new(Arc::new(RwLock::new(None)));
    let result = store2
        .initialize_session_team(bootstrap_request(&team_name, session_id))
        .await
        .expect("resume bootstrap ok");
    assert!(matches!(
        result,
        InitializeSessionTeamResult::AlreadyExists { .. }
    ));
    // The file was not clobbered — same created_at as the original.
    let after = team_file::read_team_file(&team_name)
        .expect("read ok")
        .expect("still present");
    assert_eq!(after.created_at, original.created_at);
    assert_eq!(
        store2.active_team_name().await.as_deref(),
        Some(team_name.as_str())
    );
}

#[tokio::test]
async fn initialize_session_team_rejects_empty_session_id() {
    let _teams = crate::test_support::isolate_teams_dir().await;
    let store = TeamRosterStore::new(Arc::new(RwLock::new(None)));
    let err = store
        .initialize_session_team(bootstrap_request("session-", ""))
        .await
        .expect_err("empty session id rejected");
    assert!(err.contains("non-empty leader session id"));
}
