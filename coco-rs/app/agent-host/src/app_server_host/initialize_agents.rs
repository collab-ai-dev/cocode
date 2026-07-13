use std::collections::HashMap;
use std::str::FromStr;

use coco_subagent::AgentDefinitionValidator;
use coco_types::{AgentDefinition, AgentSource, AgentTypeId, ClientAgentDefinition, ToolAllowList};

/// Parse the `agents` map from initialize params into validated definitions.
///
/// The map's keys are authoritative agent type names. Parse errors are
/// collected per entry so initialize can accept the valid subset.
pub(crate) fn parse_client_agent_definitions(
    agents: &HashMap<String, ClientAgentDefinition>,
) -> (Vec<AgentDefinition>, Vec<String>) {
    let mut accepted = Vec::with_capacity(agents.len());
    let mut errors = Vec::new();
    for (name, client_def) in agents {
        let mut def = client_agent_definition_to_internal(client_def);
        def.name = name.clone();
        def.agent_type = match AgentTypeId::from_str(name) {
            Ok(t) => t,
            Err(_) => AgentTypeId::Custom(name.clone()),
        };
        def.source = AgentSource::FlagSettings;
        let semantic_errors = AgentDefinitionValidator::check(&def);
        if !semantic_errors.is_empty() {
            errors.push(format!(
                "agent '{name}': validation failed: {}",
                semantic_errors
                    .iter()
                    .map(|e| format!("{e:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            continue;
        }
        accepted.push(def);
    }
    (accepted, errors)
}

fn client_agent_definition_to_internal(client: &ClientAgentDefinition) -> AgentDefinition {
    let allowed_tools = match client.tools.as_ref() {
        None => ToolAllowList::Wildcard,
        Some(list) => ToolAllowList::from_frontmatter(list.clone()),
    };
    let permission_mode = client.permission_mode.and_then(|m| {
        serde_json::to_value(m)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
    });
    AgentDefinition {
        description: Some(client.description.clone()),
        system_prompt: Some(client.prompt.clone()),
        allowed_tools,
        disallowed_tools: client.disallowed_tools.clone().unwrap_or_default(),
        model: client.model.clone(),
        mcp_servers: client.mcp_servers.clone().unwrap_or_default(),
        critical_system_reminder: client.critical_system_reminder_experimental.clone(),
        skills: client.skills.clone().unwrap_or_default(),
        initial_prompt: client.initial_prompt.clone(),
        max_turns: client.max_turns,
        background: client.background.unwrap_or(false),
        memory_scope: client.memory,
        effort: client.effort,
        permission_mode,
        ..AgentDefinition::default()
    }
}
