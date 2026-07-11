use std::collections::HashMap;

use coco_types::AgentDefinition;
use coco_types::HookCallbackMatcher;
use coco_types::HookEventType;
use tokio::sync::RwLock;

#[derive(Default)]
pub(super) struct InitializeState {
    sdk_agents: RwLock<Vec<AgentDefinition>>,
    plan_mode_instructions: RwLock<Option<String>>,
    hooks: RwLock<Option<HashMap<HookEventType, Vec<HookCallbackMatcher>>>>,
}

impl InitializeState {
    pub(super) async fn set_plan_mode_instructions(&self, instructions: Option<String>) {
        *self.plan_mode_instructions.write().await = instructions;
    }

    pub(super) async fn plan_mode_instructions(&self) -> Option<String> {
        self.plan_mode_instructions.read().await.clone()
    }

    pub(super) async fn set_hooks(
        &self,
        hooks: Option<HashMap<HookEventType, Vec<HookCallbackMatcher>>>,
    ) {
        *self.hooks.write().await = hooks;
    }

    pub(super) async fn hooks(&self) -> Option<HashMap<HookEventType, Vec<HookCallbackMatcher>>> {
        self.hooks.read().await.clone()
    }

    pub(super) async fn set_sdk_agents(&self, agents: Vec<AgentDefinition>) {
        *self.sdk_agents.write().await = agents;
    }

    pub(super) async fn sdk_agents(&self) -> Vec<AgentDefinition> {
        self.sdk_agents.read().await.clone()
    }
}
