use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use coco_tool_runtime::AgentHandle;
use coco_tool_runtime::AgentSpawnRequest;
use coco_tool_runtime::AgentSpawnResponse;
use coco_tool_runtime::AgentSpawnStatus;

use super::*;

#[derive(Default)]
struct SlowHandle {
    calls: AtomicUsize,
    active_calls: AtomicUsize,
    max_active_calls: AtomicUsize,
    delay: Duration,
}

impl SlowHandle {
    fn with_delay(delay: Duration) -> Self {
        Self {
            delay,
            ..Default::default()
        }
    }

    fn record_start(&self) {
        let active = self.active_calls.fetch_add(1, Ordering::SeqCst) + 1;
        let mut observed = self.max_active_calls.load(Ordering::SeqCst);
        while active > observed {
            match self.max_active_calls.compare_exchange(
                observed,
                active,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(next) => observed = next,
            }
        }
    }
}

#[async_trait::async_trait]
impl AgentHandle for SlowHandle {
    async fn spawn_agent(&self, _request: AgentSpawnRequest) -> Result<AgentSpawnResponse, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.record_start();
        tokio::time::sleep(self.delay).await;
        self.active_calls.fetch_sub(1, Ordering::SeqCst);
        Ok(AgentSpawnResponse {
            status: AgentSpawnStatus::Completed,
            agent_id: Some("slow".into()),
            result: Some("ok".into()),
            total_tool_use_count: 1,
            duration_ms: 250,
            ..Default::default()
        })
    }

    async fn send_message(
        &self,
        _to: &str,
        _content: &str,
        _summary: Option<&str>,
    ) -> Result<coco_tool_runtime::TeamMessageDispatchResult, String> {
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

fn finalize_ctx(last_id: &str) -> FinalizeTurnContext {
    FinalizeTurnContext {
        estimated_tokens: 100,
        tool_calls_since_sm_cursor: 0,
        tool_calls_last_turn: 0,
        last_message_id: Some(last_id.to_string()),
        auto_compact_enabled: false,
        bare_mode: false,
        is_subagent: false,
        now_ms: 1,
        extract_input: crate::service::extract::TurnInput {
            fork_messages: Box::new(Vec::new),
            message_count: 1,
            last_message_id: Some(last_id.to_string()),
            has_memory_writes: Box::new(|| false),
        },
        recent_tool_writes: Vec::new(),
    }
}

#[tokio::test]
async fn finalize_turn_schedules_memory_work_without_waiting_for_agent() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let agent = Arc::new(SlowHandle::with_delay(Duration::from_millis(250)));
    let mut config = MemoryConfig {
        dream_enabled: false,
        session_memory_enabled: false,
        extraction_throttle: 1,
        ..Default::default()
    };
    config.directory = Some(tmp.path().join("memory"));
    let runtime = MemoryRuntimeBuilder::new(
        tmp.path().join("home"),
        tmp.path().join("project"),
        "session-1",
        config,
        agent.clone(),
    )
    .build();

    let started = tokio::time::Instant::now();
    let report = tokio::time::timeout(
        Duration::from_millis(80),
        runtime.finalize_turn(finalize_ctx("msg-1")),
    )
    .await
    .expect("finalize_turn should not wait for slow memory agent");

    assert!(!report.skipped);
    assert!(
        started.elapsed() < Duration::from_millis(120),
        "finalize_turn waited for background work"
    );
    assert!(runtime.drain(Duration::from_secs(2)).await);
    assert_eq!(agent.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn finalize_turn_coalesces_pending_background_work() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let agent = Arc::new(SlowHandle::with_delay(Duration::from_millis(250)));
    let mut config = MemoryConfig {
        dream_enabled: false,
        session_memory_enabled: false,
        extraction_throttle: 1,
        ..Default::default()
    };
    config.directory = Some(tmp.path().join("memory"));
    let runtime = MemoryRuntimeBuilder::new(
        tmp.path().join("home"),
        tmp.path().join("project"),
        "session-1",
        config,
        agent.clone(),
    )
    .build();

    let _ = runtime.finalize_turn(finalize_ctx("msg-1")).await;
    let _ = runtime.finalize_turn(finalize_ctx("msg-2")).await;
    let _ = runtime.finalize_turn(finalize_ctx("msg-3")).await;

    assert!(runtime.drain(Duration::from_secs(2)).await);
    assert_eq!(agent.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn subagent_finalize_turn_does_not_classify_or_drain_memory_notices() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let agent = Arc::new(SlowHandle::with_delay(Duration::from_millis(10)));
    let mut config = MemoryConfig {
        dream_enabled: false,
        session_memory_enabled: false,
        extraction_throttle: 1,
        ..Default::default()
    };
    config.directory = Some(tmp.path().join("memory"));
    let runtime = MemoryRuntimeBuilder::new(
        tmp.path().join("home"),
        tmp.path().join("project"),
        "session-1",
        config,
        agent,
    )
    .build();

    let mut ctx = finalize_ctx("msg-1");
    ctx.is_subagent = true;
    ctx.recent_tool_writes = vec![ToolWriteRecord {
        tool_name: "Write".into(),
        file_path: runtime.personal_dir().join("notes.md"),
        succeeded: true,
    }];

    let report = runtime.finalize_turn(ctx).await;

    assert!(report.skipped);
    assert!(report.notices.is_empty());
    assert!(runtime.drain_user_notices().is_empty());
}

#[tokio::test]
async fn drain_returns_after_extract_without_waiting_for_auto_dream() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let agent = Arc::new(SlowHandle::with_delay(Duration::from_millis(250)));
    let mut config = MemoryConfig {
        dream_enabled: true,
        dream_min_hours: 1,
        dream_min_sessions: 1,
        session_memory_enabled: false,
        extraction_throttle: 1,
        ..Default::default()
    };
    config.directory = Some(tmp.path().join("memory"));
    let runtime = MemoryRuntimeBuilder::new(
        tmp.path().join("home"),
        tmp.path().join("project"),
        "session-1",
        config,
        agent.clone(),
    )
    .with_transcript_dir(tmp.path().join("transcripts"))
    .build();
    assert!(
        runtime
            .install_session_enumerator(Arc::new(|| vec!["s1".to_string()]))
            .is_ok()
    );

    let _ = runtime.finalize_turn(finalize_ctx("msg-1")).await;
    let started = tokio::time::Instant::now();

    assert!(runtime.drain(Duration::from_secs(2)).await);
    assert!(
        started.elapsed() < Duration::from_millis(450),
        "drain waited for auto-dream instead of extraction"
    );
    assert!(runtime.drain_all(Duration::from_secs(2)).await);
    assert_eq!(agent.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn extract_and_dream_do_not_overlap_durable_memory_writes() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let agent = Arc::new(SlowHandle::with_delay(Duration::from_millis(120)));
    let mut config = MemoryConfig {
        dream_enabled: true,
        dream_min_hours: 1,
        dream_min_sessions: 1,
        session_memory_enabled: false,
        extraction_throttle: 1,
        ..Default::default()
    };
    config.directory = Some(tmp.path().join("memory"));
    let runtime = MemoryRuntimeBuilder::new(
        tmp.path().join("home"),
        tmp.path().join("project"),
        "session-1",
        config,
        agent.clone(),
    )
    .with_transcript_dir(tmp.path().join("transcripts"))
    .build();
    assert!(
        runtime
            .install_session_enumerator(Arc::new(|| vec!["s1".to_string()]))
            .is_ok()
    );

    let _ = runtime.finalize_turn(finalize_ctx("msg-1")).await;

    assert!(runtime.drain_all(Duration::from_secs(2)).await);
    assert_eq!(agent.calls.load(Ordering::SeqCst), 2);
    assert_eq!(agent.max_active_calls.load(Ordering::SeqCst), 1);
}
