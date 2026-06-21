use coco_types::{AgentDefinition, AgentTypeId, LlmModelSelection, ModelRole, SubagentType};
use pretty_assertions::assert_eq;

use super::{SubagentSelection, resolve_subagent_selection};

#[test]
fn builtin_explore_maps_to_explore_role() {
    let id = AgentTypeId::Builtin(SubagentType::Explore);
    let sel = resolve_subagent_selection(None, Some(&id));
    assert_eq!(
        sel,
        SubagentSelection {
            model: None,
            model_role: ModelRole::Explore,
            model_selection: LlmModelSelection::Role {
                role: ModelRole::Explore,
            },
        }
    );
}

#[test]
fn builtin_plan_maps_to_plan_role() {
    let id = AgentTypeId::Builtin(SubagentType::Plan);
    let sel = resolve_subagent_selection(None, Some(&id));
    assert_eq!(
        sel,
        SubagentSelection {
            model: None,
            model_role: ModelRole::Plan,
            model_selection: LlmModelSelection::Role {
                role: ModelRole::Plan,
            },
        }
    );
}

#[test]
fn definition_model_role_wins_over_subagent_type_mapping() {
    let def = AgentDefinition {
        model_role: Some(ModelRole::Fast),
        ..Default::default()
    };
    let id = AgentTypeId::Builtin(SubagentType::Explore);
    let sel = resolve_subagent_selection(Some(&def), Some(&id));
    assert_eq!(sel.model_role, ModelRole::Fast);
    assert_eq!(
        sel.model_selection,
        LlmModelSelection::Role {
            role: ModelRole::Fast,
        }
    );
}

#[test]
fn definition_provider_model_id_becomes_explicit_selection() {
    let def = AgentDefinition {
        model: Some("anthropic/claude-haiku-4-5".into()),
        model_role: Some(ModelRole::Review),
        ..Default::default()
    };
    let id = AgentTypeId::Builtin(SubagentType::Explore);
    let sel = resolve_subagent_selection(Some(&def), Some(&id));
    assert_eq!(
        sel,
        SubagentSelection {
            model: Some("anthropic/claude-haiku-4-5".into()),
            model_role: ModelRole::Review,
            model_selection: LlmModelSelection::ExplicitWithFallbackRole {
                primary: coco_types::ProviderModelSelection {
                    provider: "anthropic".into(),
                    model_id: "claude-haiku-4-5".into(),
                },
                fallback_role: ModelRole::Review,
            },
        }
    );
}

#[test]
fn definition_inherit_model_is_explicit_inherit_main() {
    let def = AgentDefinition {
        model: Some("inherit".into()),
        model_role: Some(ModelRole::Plan),
        ..Default::default()
    };
    let sel = resolve_subagent_selection(Some(&def), None);
    assert_eq!(sel.model.as_deref(), Some("inherit"));
    assert_eq!(sel.model_role, ModelRole::Plan);
    assert_eq!(sel.model_selection, LlmModelSelection::InheritMain);
}

#[test]
fn custom_agent_without_definition_falls_back_to_subagent_role() {
    let id = AgentTypeId::Custom("research-bot".into());
    let sel = resolve_subagent_selection(None, Some(&id));
    assert_eq!(sel.model_role, ModelRole::Subagent);
    assert!(sel.model.is_none());
    assert_eq!(
        sel.model_selection,
        LlmModelSelection::Role {
            role: ModelRole::Subagent,
        }
    );
}

#[test]
fn verification_subagent_maps_to_review_role() {
    let id = AgentTypeId::Builtin(SubagentType::Verification);
    let sel = resolve_subagent_selection(None, Some(&id));
    assert_eq!(sel.model_role, ModelRole::Review);
}
