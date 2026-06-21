//! Shared test fakes for the in-crate `*.test.rs` files.
//!
//! `RecordingHandle` is a minimal `AgentHandle` that captures every
//! spawn request so service tests can inspect which constraints,
//! prompts, and fork-context messages get sent. Identical copies
//! used to live in three test files; consolidated here.

use std::sync::Mutex;

use coco_tool_runtime::AgentHandle;
use coco_tool_runtime::AgentSpawnRequest;
use coco_tool_runtime::AgentSpawnResponse;
use coco_tool_runtime::AgentSpawnStatus;

/// Records every `spawn_agent` request and returns a static canned
/// response (`Completed` with `tool_use_count = 1`) — enough for
/// "fired vs skipped" assertions. If a test needs a different
/// response shape, build a different fake; this one stays minimal.
#[derive(Default)]
pub(crate) struct RecordingHandle {
    pub(crate) inner: Mutex<Vec<AgentSpawnRequest>>,
}

impl RecordingHandle {
    pub(crate) fn calls(&self) -> Vec<AgentSpawnRequest> {
        self.inner.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl AgentHandle for RecordingHandle {
    async fn spawn_agent(&self, request: AgentSpawnRequest) -> Result<AgentSpawnResponse, String> {
        self.inner.lock().unwrap().push(request);
        Ok(AgentSpawnResponse {
            status: AgentSpawnStatus::Completed,
            agent_id: Some("test".into()),
            result: Some("ok".into()),
            total_tool_use_count: 1,
            duration_ms: 1,
            ..Default::default()
        })
    }
    async fn send_message(&self, _to: &str, _content: &str) -> Result<String, String> {
        Err("unused".into())
    }
    async fn create_team(
        &self,
        _request: coco_tool_runtime::CreateTeamRequest,
    ) -> Result<coco_tool_runtime::CreateTeamResult, String> {
        Err("unused".into())
    }
    async fn delete_team(&self) -> Result<String, String> {
        Err("unused".into())
    }
    async fn resume_agent(
        &self,
        _agent_id: &str,
        _prompt: &str,
        _session_id: &str,
    ) -> Result<AgentSpawnResponse, String> {
        Err("unused".into())
    }
    async fn query_agent_status(&self, _agent_id: &str) -> Result<AgentSpawnResponse, String> {
        Err("unused".into())
    }
    async fn get_agent_output(&self, _agent_id: &str) -> Result<String, String> {
        Err("unused".into())
    }
}
