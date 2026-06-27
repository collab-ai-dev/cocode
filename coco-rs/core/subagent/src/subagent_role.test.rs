use coco_types::{AgentDefinition, AgentTypeId, ModelRole, SubagentType};
use pretty_assertions::assert_eq;

use super::{resolve_subagent_role, role_for_builtin};

#[test]
fn test_role_for_builtin_explore() {
    assert_eq!(role_for_builtin(SubagentType::Explore), ModelRole::Explore);
}

#[test]
fn test_role_for_builtin_plan() {
    assert_eq!(role_for_builtin(SubagentType::Plan), ModelRole::Plan);
}

#[test]
fn test_role_for_builtin_verification_maps_to_review() {
    assert_eq!(
        role_for_builtin(SubagentType::Verification),
        ModelRole::Review
    );
}

#[test]
fn test_role_for_builtin_general_purpose_falls_through_to_subagent() {
    assert_eq!(
        role_for_builtin(SubagentType::GeneralPurpose),
        ModelRole::Subagent
    );
    assert_eq!(
        role_for_builtin(SubagentType::StatusLine),
        ModelRole::Subagent
    );
    assert_eq!(
        role_for_builtin(SubagentType::CocoGuide),
        ModelRole::Subagent
    );
}

#[test]
fn test_resolve_uses_definition_model_role_when_set() {
    let def = AgentDefinition {
        model_role: Some(ModelRole::Fast),
        ..Default::default()
    };
    let id = AgentTypeId::Builtin(SubagentType::Explore);
    // Definition's `Fast` wins over Explore's default `Explore` role.
    assert_eq!(
        resolve_subagent_role(Some(&def), Some(&id)),
        ModelRole::Fast
    );
}

#[test]
fn test_resolve_falls_back_to_subagent_type_when_definition_absent() {
    let id = AgentTypeId::Builtin(SubagentType::Plan);
    assert_eq!(resolve_subagent_role(None, Some(&id)), ModelRole::Plan);
}

#[test]
fn test_resolve_falls_back_to_subagent_when_definition_role_unset() {
    let def = AgentDefinition::default();
    let id = AgentTypeId::Builtin(SubagentType::Verification);
    assert_eq!(
        resolve_subagent_role(Some(&def), Some(&id)),
        ModelRole::Review
    );
}

#[test]
fn test_resolve_custom_agent_without_definition_role() {
    let def = AgentDefinition {
        agent_type: AgentTypeId::Custom("my-special-agent".into()),
        ..Default::default()
    };
    let id = AgentTypeId::Custom("my-special-agent".into());
    assert_eq!(
        resolve_subagent_role(Some(&def), Some(&id)),
        ModelRole::Subagent
    );
}

#[test]
fn test_resolve_custom_agent_with_definition_role() {
    let def = AgentDefinition {
        agent_type: AgentTypeId::Custom("review-bot".into()),
        model_role: Some(ModelRole::Review),
        ..Default::default()
    };
    let id = AgentTypeId::Custom("review-bot".into());
    assert_eq!(
        resolve_subagent_role(Some(&def), Some(&id)),
        ModelRole::Review
    );
}

#[test]
fn test_resolve_no_inputs_defaults_to_subagent() {
    assert_eq!(resolve_subagent_role(None, None), ModelRole::Subagent);
}
