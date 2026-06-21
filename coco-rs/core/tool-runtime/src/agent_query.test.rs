use super::*;

#[test]
fn agent_run_identity_rejects_empty_session_id() {
    let err = AgentRunIdentity::new("", "agent-1", AgentRunKind::Subagent)
        .expect_err("empty session_id must be rejected");
    assert!(err.contains("session_id"));
}

#[test]
fn agent_run_identity_rejects_empty_agent_id() {
    let err = AgentRunIdentity::new("session-1", " ", AgentRunKind::Subagent)
        .expect_err("empty agent_id must be rejected");
    assert!(err.contains("agent_id"));
}

#[test]
fn agent_query_config_defaults_are_test_only_fail_closed() {
    let cfg = AgentQueryConfig::default();
    assert_eq!(cfg.identity.kind, AgentRunKind::Test);
    assert_eq!(
        cfg.model_selection,
        coco_types::LlmModelSelection::InheritMain
    );
    assert_eq!(
        cfg.permission_prompt_policy,
        PermissionPromptPolicy::FailClosed
    );
}
