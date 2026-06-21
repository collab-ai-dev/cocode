use super::*;

// ── Model Fallback ──

#[test]
fn test_resolve_teammate_model_explicit() {
    let model = resolve_teammate_model(
        Some("opus-4"),
        "leader-model",
        &coco_config::AgentTeamsConfig::default(),
        None,
        |_| None,
    );
    assert_eq!(model.model, "opus-4");
    assert_eq!(model.model_role, None);
}

#[test]
fn test_resolve_teammate_model_inherit() {
    let model = resolve_teammate_model(
        Some("inherit"),
        "leader-model",
        &coco_config::AgentTeamsConfig::default(),
        None,
        |_| None,
    );
    assert_eq!(model.model, "leader-model");
    assert_eq!(model.model_role, Some(coco_types::ModelRole::Main));
}

#[test]
fn test_resolve_teammate_model_config_default() {
    let config = coco_config::AgentTeamsConfig {
        default_model: Some(coco_types::ProviderModelSelection {
            provider: "openai".into(),
            model_id: "config-default".into(),
        }),
        ..Default::default()
    };
    let model = resolve_teammate_model(None, "leader", &config, None, |_| None);
    assert_eq!(model.model, "config-default");
    assert_eq!(model.model_role, None);
}

#[test]
fn test_resolve_teammate_model_agent_type_role_overrides_default() {
    let config = coco_config::AgentTeamsConfig {
        default_model_role: coco_types::ModelRole::Fast,
        agent_type_model_roles: [("reviewer".into(), coco_types::ModelRole::Review)]
            .into_iter()
            .collect(),
        ..Default::default()
    };
    let model = resolve_teammate_model(None, "leader-model", &config, Some("reviewer"), |role| {
        Some(format!("{role}-model"))
    });
    assert_eq!(model.model, "review-model");
    assert_eq!(model.model_role, Some(coco_types::ModelRole::Review));
}

#[test]
fn test_resolve_teammate_model_missing_config_uses_main_role() {
    let model = resolve_teammate_model(
        None,
        "leader-model",
        &coco_config::AgentTeamsConfig::default(),
        None,
        |role| Some(format!("{role}-model")),
    );
    assert_eq!(model.model, "main-model");
    assert_eq!(model.model_role, Some(coco_types::ModelRole::Main));
}

// ── Mode Snapshot ──

#[test]
fn test_mode_snapshot_default() {
    // Without capture, returns Auto
    let mode = get_teammate_mode_from_snapshot();
    // May be Auto or previously set value (global state)
    let _ = mode;
}

#[test]
fn test_mode_snapshot_capture() {
    capture_teammate_mode_snapshot(TeammateMode::Tmux);
    assert_eq!(get_teammate_mode_from_snapshot(), TeammateMode::Tmux);

    // Reset
    capture_teammate_mode_snapshot(TeammateMode::Auto);
}

#[test]
fn test_cli_mode_override() {
    set_cli_teammate_mode_override(TeammateMode::InProcess);
    assert_eq!(
        get_cli_teammate_mode_override(),
        Some(TeammateMode::InProcess)
    );

    // Capture should use CLI override
    capture_teammate_mode_snapshot(TeammateMode::Tmux);
    assert_eq!(get_teammate_mode_from_snapshot(), TeammateMode::InProcess);

    // Clean up
    if let Ok(mut guard) = super::CLI_MODE_OVERRIDE.write() {
        *guard = None;
    }
    capture_teammate_mode_snapshot(TeammateMode::Auto);
}

// ── Spawn Helpers ──

#[test]
fn test_generate_unique_teammate_name_no_collision() {
    let name = generate_unique_teammate_name("researcher", &[]);
    assert_eq!(name, "researcher");
}

#[test]
fn test_generate_unique_teammate_name_with_collision() {
    let existing = vec!["researcher".to_string()];
    let name = generate_unique_teammate_name("researcher", &existing);
    assert_eq!(name, "researcher-2");
}

#[test]
fn test_generate_unique_teammate_name_multiple_collisions() {
    let existing = vec![
        "worker".to_string(),
        "worker-2".to_string(),
        "worker-3".to_string(),
    ];
    let name = generate_unique_teammate_name("worker", &existing);
    assert_eq!(name, "worker-4");
}

// ── Message Formatting ──

#[test]
fn test_format_as_teammate_message_basic() {
    let msg = format_as_teammate_message("worker", "Hello leader", None, None);
    assert!(msg.contains("teammate_message"));
    assert!(msg.contains("teammate_id=\"worker\""));
    assert!(msg.contains("Hello leader"));
    assert!(!msg.contains("color="));
}

#[test]
fn test_format_as_teammate_message_with_attrs() {
    let msg = format_as_teammate_message("worker", "Done", Some("blue"), Some("task done"));
    assert!(msg.contains("color=\"blue\""));
    assert!(msg.contains("summary=\"task done\""));
}

// ── Message Priority ──

#[test]
fn test_message_priority_ordering() {
    assert!(MessagePriority::PendingUserMessage < MessagePriority::ShutdownRequest);
    assert!(MessagePriority::ShutdownRequest < MessagePriority::LeaderMessage);
    assert!(MessagePriority::LeaderMessage < MessagePriority::PeerMessage);
    assert!(MessagePriority::PeerMessage < MessagePriority::UnclaimedTask);
}
