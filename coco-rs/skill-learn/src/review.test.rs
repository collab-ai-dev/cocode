use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use async_trait::async_trait;
use coco_tool_runtime::{AgentHandle, AgentHandleRef, AgentSpawnRequest, AgentSpawnResponse};
use coco_types::{ForkLabel, ModelRole, SessionId};

use super::{AgentSlot, SkillReviewOutcome, SkillReviewService};

/// Records the last spawn request so tests can assert its shape.
#[derive(Default)]
struct CapturingHandle {
    last: Mutex<Option<AgentSpawnRequest>>,
}

#[async_trait]
impl AgentHandle for CapturingHandle {
    async fn spawn_agent(&self, request: AgentSpawnRequest) -> Result<AgentSpawnResponse, String> {
        *self.last.lock().unwrap() = Some(request);
        Ok(AgentSpawnResponse {
            paths_written: vec![PathBuf::from("/x/.agent/s/SKILL.md")],
            ..Default::default()
        })
    }

    async fn send_message(
        &self,
        _to: &str,
        _message: &str,
        _from: Option<&str>,
    ) -> Result<coco_tool_runtime::TeamMessageDispatchResult, String> {
        Err("unused".into())
    }

    async fn query_agent_status(&self, _id: &str) -> Result<AgentSpawnResponse, String> {
        Ok(AgentSpawnResponse::default())
    }

    async fn get_agent_output(&self, _id: &str) -> Result<String, String> {
        Ok(String::new())
    }
}

#[tokio::test]
async fn review_fork_uses_memory_role_fence_and_label() {
    let tmp = tempfile::tempdir().unwrap();
    let handle = Arc::new(CapturingHandle::default());
    let slot: AgentSlot = Arc::new(RwLock::new(handle.clone() as AgentHandleRef));
    let svc = SkillReviewService::new(slot, tmp.path());

    let session_id = SessionId::try_new("sess-1").unwrap();
    let outcome = svc.run(session_id.clone(), Vec::new()).await;
    assert_eq!(outcome, SkillReviewOutcome::Completed { paths_written: 1 });

    let req = handle
        .last
        .lock()
        .unwrap()
        .take()
        .expect("spawn_agent should have been called");

    // Routing + fencing invariants.
    assert_eq!(req.telemetry.fork_label, Some(ForkLabel::SkillReview));
    assert_eq!(req.routing.session_id, Some(session_id));
    assert!(
        req.execution.skip_transcript,
        "review fork must not pollute transcript"
    );
    assert!(
        req.permissions.require_can_use_tool,
        "fence must run even under a hook Allow"
    );
    assert!(
        req.permissions.can_use_tool.is_some(),
        "write fence must be installed"
    );

    let def = req.input.definition.expect("synthetic definition");
    assert_eq!(
        def.model_role,
        Some(ModelRole::Memory),
        "background self-improvement routes to the memory role, not review"
    );

    let constraints = req.permissions.constraints.expect("write-root constraints");
    assert!(
        constraints
            .allowed_write_roots
            .iter()
            .any(|p| p.ends_with(".agent")),
        "outer-ring fence must be the agent skills dir"
    );
}

#[tokio::test]
async fn review_fork_surfaces_spawn_failure() {
    let tmp = tempfile::tempdir().unwrap();
    // The no-op handle's spawn_agent always errors — the built-in failure double.
    let slot: AgentSlot = Arc::new(RwLock::new(
        Arc::new(coco_tool_runtime::NoOpAgentHandle) as AgentHandleRef
    ));
    let svc = SkillReviewService::new(slot, tmp.path());
    let outcome = svc.run(SessionId::try_new("s").unwrap(), Vec::new()).await;
    assert!(
        matches!(outcome, SkillReviewOutcome::Failed { .. }),
        "spawn failure must surface as Failed, got {outcome:?}"
    );
}
