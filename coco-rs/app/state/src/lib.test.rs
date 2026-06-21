use crate::*;
use coco_types::PermissionMode;

#[tokio::test]
async fn test_create_default_state() {
    let store = create_app_state();
    let state = store.read().await;
    assert!(!state.is_busy);
    assert_eq!(state.turn_count, 0);
    assert_eq!(state.total_input_tokens, 0);
}

#[tokio::test]
async fn test_create_with_config() {
    let store = create_app_state_with("opus", "/tmp", PermissionMode::Default);
    let state = store.read().await;
    assert_eq!(state.model, "opus");
    assert_eq!(state.working_dir, "/tmp");
}

#[tokio::test]
async fn test_update_token_usage() {
    let store = create_app_state();
    update_token_usage(&store, 100, 50).await;
    update_token_usage(&store, 200, 100).await;
    let state = store.read().await;
    assert_eq!(state.total_input_tokens, 300);
    assert_eq!(state.total_output_tokens, 150);
}

#[tokio::test]
async fn test_set_busy() {
    let store = create_app_state();
    set_busy(&store, true, Some("Read")).await;
    {
        let state = store.read().await;
        assert!(state.is_busy);
        assert_eq!(state.current_tool.as_deref(), Some("Read"));
    }
    set_busy(&store, false, None).await;
    let state = store.read().await;
    assert!(!state.is_busy);
    assert!(state.current_tool.is_none());
}

#[tokio::test]
async fn test_increment_turn() {
    let store = create_app_state();
    increment_turn(&store).await;
    increment_turn(&store).await;
    let state = store.read().await;
    assert_eq!(state.turn_count, 2);
}
