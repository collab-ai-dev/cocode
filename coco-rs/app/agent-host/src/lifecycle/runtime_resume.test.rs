use std::path::Path;
use std::sync::Arc;

use super::*;

fn test_session_id(value: &str) -> coco_types::SessionId {
    coco_types::SessionId::try_new(value).expect("valid test session id")
}

fn seed_titled_session(memory_base: &Path, cwd: &Path, session_id: &str, title: &str) {
    std::fs::create_dir_all(cwd).expect("create test cwd");
    let paths = Arc::new(coco_paths::ProjectPaths::new(
        memory_base.to_path_buf(),
        cwd,
    ));
    let store = coco_session::TranscriptStore::new(paths);
    let typed_session_id = test_session_id(session_id);
    let entry = coco_session::TranscriptEntry {
        entry_type: "user".to_string(),
        uuid: format!("{session_id}-u1"),
        parent_uuid: None,
        logical_parent_uuid: None,
        session_id: Some(typed_session_id.clone()),
        cwd: cwd.display().to_string(),
        timestamp: "2025-01-15T10:00:00Z".to_string(),
        version: Some("1.0.0".to_string()),
        git_branch: None,
        is_sidechain: false,
        agent_id: None,
        message: Some(serde_json::json!({
            "role": "user",
            "content": [{"type": "text", "text": "hi"}],
        })),
        usage: None,
        model: None,
        request_id: None,
        cost_usd: None,
        extra: serde_json::Map::new(),
    };
    store
        .append_message(session_id, &entry)
        .expect("append test transcript");
    store
        .append_metadata(
            session_id,
            &coco_session::MetadataEntry::CustomTitle {
                session_id: typed_session_id,
                custom_title: title.to_string(),
            },
        )
        .expect("append title metadata");
}

fn empty_resume_conversation() -> coco_session::recovery::ConversationForResume {
    coco_session::recovery::ConversationForResume {
        messages: Vec::new(),
        model: "seed-model".to_string(),
        turn_count: 0,
        total_input_tokens: 0,
        total_output_tokens: 0,
        turn_interruption_state: coco_messages::TurnInterruptionState::None,
        plan_slug: None,
        has_sidechain: false,
        mode: None,
        mcp_tool_exposure: None,
    }
}

#[tokio::test]
async fn resume_plan_session_seq_watermark_reads_destination_transcript() {
    let home = tempfile::tempdir().expect("tempdir");
    let cwd = home.path().join("project");
    std::fs::create_dir_all(&cwd).expect("create cwd");
    let session_id = test_session_id("sess-resume-watermark");
    let store = coco_session::TranscriptStore::new(Arc::new(coco_paths::ProjectPaths::new(
        home.path().to_path_buf(),
        &cwd,
    )));
    store
        .append_metadata(
            session_id.as_str(),
            &coco_session::MetadataEntry::SessionSeqWatermark {
                session_id: session_id.clone(),
                session_seq: 40,
            },
        )
        .expect("append watermark");
    let transcript_path = store.transcript_path(session_id.as_str());
    let plan = ResumePlan {
        session_id: session_id.clone(),
        source_session_id: session_id,
        source_path: transcript_path.clone(),
        destination_path: transcript_path,
        cwd,
        prior_messages: Vec::new(),
        conversation: empty_resume_conversation(),
        is_fork: false,
    };

    assert_eq!(resume_plan_session_seq_watermark(&plan).await, Some(40));
}

#[test]
fn title_resolution_filters_matches_to_runtime_project() {
    let home = tempfile::tempdir().expect("tempdir");
    let project_a = home.path().join("project-a");
    let project_b = home.path().join("project-b");
    seed_titled_session(home.path(), &project_a, "sess-project-a", "Daily");
    seed_titled_session(home.path(), &project_b, "sess-project-b", "Daily");
    let manager = coco_session::SessionManager::new(home.path().to_path_buf());
    let id_err = manager
        .resume("Daily")
        .expect_err("title is not a direct session id");

    let resolved = resolve_resume_target_by_title(
        &manager,
        "Daily",
        &crate::paths::resolve_project_root(&project_a),
        &id_err,
    )
    .expect("same-project title should resolve");

    assert_eq!(resolved.id, "sess-project-a");
    assert_eq!(resolved.working_dir, project_a);
}
