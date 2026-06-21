use super::*;

fn temp_store() -> SessionAgentTranscriptStore {
    let dir = std::env::temp_dir().join(format!("coco-trans-{}", uuid::Uuid::new_v4().simple()));
    let paths = Arc::new(coco_paths::ProjectPaths::new(
        dir,
        std::path::Path::new("/test-project"),
    ));
    let store = Arc::new(TranscriptStore::new(paths));
    SessionAgentTranscriptStore::new(store)
}

#[tokio::test]
async fn write_then_read_metadata_roundtrip() {
    let st = temp_store();
    let meta = AgentSpawnMetadata {
        agent_type: "Explore".into(),
        worktree_path: Some("/tmp/wt".into()),
        description: Some("look around".into()),
    };
    st.write_agent_metadata("sess-1", "agent-7af2", &meta)
        .await
        .unwrap();
    let got = st
        .read_agent_metadata("sess-1", "agent-7af2")
        .await
        .unwrap();
    assert_eq!(got, Some(meta));
}

#[tokio::test]
async fn read_metadata_returns_none_for_unknown() {
    let st = temp_store();
    let got = st.read_agent_metadata("sess-1", "ghost").await.unwrap();
    assert!(got.is_none());
}

#[tokio::test]
async fn append_then_load_messages_roundtrip() {
    let st = temp_store();
    let msgs = vec![
        Arc::new(coco_messages::create_user_message("hello")),
        Arc::new(coco_messages::create_user_message("hi")),
    ];
    st.append_agent_messages("sess-1", "agent-1", &msgs)
        .await
        .unwrap();
    let got = st
        .load_agent_messages("sess-1", "agent-1")
        .await
        .unwrap()
        .expect("should have messages");
    assert_eq!(got.len(), 2);
    assert!(matches!(got[0].as_ref(), coco_messages::Message::User(_)));
    assert!(matches!(got[1].as_ref(), coco_messages::Message::User(_)));
}

#[tokio::test]
async fn load_messages_returns_none_for_unknown() {
    let st = temp_store();
    let got = st.load_agent_messages("sess-1", "ghost").await.unwrap();
    assert!(got.is_none());
}

#[tokio::test]
async fn append_is_additive_across_calls() {
    let st = temp_store();
    let first = vec![Arc::new(coco_messages::create_user_message("step 1"))];
    let second = vec![Arc::new(coco_messages::create_user_message("step 2"))];
    st.append_agent_messages("sess-1", "agent-1", &first)
        .await
        .unwrap();
    st.append_agent_messages("sess-1", "agent-1", &second)
        .await
        .unwrap();
    let got = st
        .load_agent_messages("sess-1", "agent-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.len(), 2);
}
