//! No-op source implementations.
//!
//! Used as test defaults and by callers that want the reminder pipeline
//! to succeed while returning empty data. Each NoOp returns the
//! neutral default for its trait's return type, never errors, never
//! blocks.

use std::collections::HashMap;

use async_trait::async_trait;

use super::traits::DiagnosticsSource;
use super::traits::HookEventsSource;
use super::traits::IdeBridgeSource;
use super::traits::McpSource;
use super::traits::MemorySource;
use super::traits::ReminderSources;
use super::traits::SkillsSource;
use super::traits::SwarmSource;
use super::traits::TaskStatusSource;
use crate::generator::AgentPendingMessage;
use crate::generator::DiagnosticFileSummary;
use crate::generator::HookEvent;
use crate::generator::InvokedSkillEntry;
use crate::generator::TaskStatusSnapshot;
use crate::generator::TeamContextSnapshot;
use crate::generator::TeammateMailboxInfo;
use crate::generators::memory::NestedMemoryInfo;
use crate::generators::memory::RelevantMemoryInfo;
use crate::generators::user_input::IdeOpenedFileSnapshot;
use crate::generators::user_input::IdeSelectionSnapshot;
use crate::generators::user_input::McpResourceEntry;

#[derive(Debug, Default)]
pub struct NoOpHookEventsSource;

#[async_trait]
impl HookEventsSource for NoOpHookEventsSource {
    async fn drain(&self, _agent_id: Option<&str>) -> Vec<HookEvent> {
        Vec::new()
    }
}

#[derive(Debug, Default)]
pub struct NoOpDiagnosticsSource;

#[async_trait]
impl DiagnosticsSource for NoOpDiagnosticsSource {
    async fn snapshot(&self, _agent_id: Option<&str>) -> Vec<DiagnosticFileSummary> {
        Vec::new()
    }
}

#[derive(Debug, Default)]
pub struct NoOpTaskStatusSource;

#[async_trait]
impl TaskStatusSource for NoOpTaskStatusSource {
    async fn collect(
        &self,
        _agent_id: Option<&str>,
        _just_compacted: bool,
    ) -> Vec<TaskStatusSnapshot> {
        Vec::new()
    }
}

#[derive(Debug, Default)]
pub struct NoOpSkillsSource;

#[async_trait]
impl SkillsSource for NoOpSkillsSource {
    async fn listing(
        &self,
        _agent_id: Option<&str>,
        _tiers: &coco_config::SkillOverrideTiers,
    ) -> Option<String> {
        None
    }
    async fn invoked(&self, _agent_id: Option<&str>) -> Vec<InvokedSkillEntry> {
        Vec::new()
    }
    async fn activate_skills_for_paths(
        &self,
        _file_paths: &[std::path::PathBuf],
        _cwd: &std::path::Path,
    ) -> Vec<String> {
        Vec::new()
    }
}

#[derive(Debug, Default)]
pub struct NoOpMcpSource;

#[async_trait]
impl McpSource for NoOpMcpSource {
    async fn instructions(&self, _agent_id: Option<&str>) -> HashMap<String, String> {
        HashMap::new()
    }
    async fn resolve_resources(
        &self,
        _agent_id: Option<&str>,
        _input: &str,
    ) -> Vec<McpResourceEntry> {
        Vec::new()
    }
}

#[derive(Debug, Default)]
pub struct NoOpSwarmSource;

#[async_trait]
impl SwarmSource for NoOpSwarmSource {
    async fn teammate_mailbox(&self, _agent_id: Option<&str>) -> Option<TeammateMailboxInfo> {
        None
    }
    async fn team_context(&self, _agent_id: Option<&str>) -> Option<TeamContextSnapshot> {
        None
    }
    async fn agent_pending_messages(&self, _agent_id: Option<&str>) -> Vec<AgentPendingMessage> {
        Vec::new()
    }
}

#[derive(Debug, Default)]
pub struct NoOpIdeBridgeSource;

#[async_trait]
impl IdeBridgeSource for NoOpIdeBridgeSource {
    async fn selection(&self, _agent_id: Option<&str>) -> Option<IdeSelectionSnapshot> {
        None
    }
    async fn opened_file(&self, _agent_id: Option<&str>) -> Option<IdeOpenedFileSnapshot> {
        None
    }
}

#[derive(Debug, Default)]
pub struct NoOpMemorySource;

#[async_trait]
impl MemorySource for NoOpMemorySource {
    async fn nested_memories(
        &self,
        _agent_id: Option<&str>,
        _mentioned_paths: &[std::path::PathBuf],
    ) -> Vec<NestedMemoryInfo> {
        Vec::new()
    }
    async fn relevant_memories(
        &self,
        _agent_id: Option<&str>,
        _input: &str,
        _recent_tools: &[String],
    ) -> Vec<RelevantMemoryInfo> {
        Vec::new()
    }
}

impl ReminderSources {
    /// Fully-populated `ReminderSources` where every slot is a NoOp.
    /// Useful for tests that want to exercise the full `materialize()`
    /// path while asserting "no reminder fired due to empty data".
    pub fn noop() -> Self {
        use std::sync::Arc;
        Self {
            hook_events: Some(Arc::new(NoOpHookEventsSource)),
            diagnostics: Some(Arc::new(NoOpDiagnosticsSource)),
            task_status: Some(Arc::new(NoOpTaskStatusSource)),
            skills: Some(Arc::new(NoOpSkillsSource)),
            mcp: Some(Arc::new(NoOpMcpSource)),
            swarm: Some(Arc::new(NoOpSwarmSource)),
            ide: Some(Arc::new(NoOpIdeBridgeSource)),
            memory: Some(Arc::new(NoOpMemorySource)),
        }
    }
}
