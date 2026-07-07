use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use coco_tool_runtime::AgentHandle;
use coco_tool_runtime::AgentHandleRef;
use coco_tool_runtime::AgentSpawnRequest;
use coco_tool_runtime::AgentSpawnResponse;
use coco_tool_runtime::AgentSpawnStatus;
use coco_types::ToolId;
use coco_types::ToolName;
use coco_types::ToolOverrides;

use super::*;

#[derive(Default)]
struct SlowHandle {
    calls: AtomicUsize,
    active_calls: AtomicUsize,
    max_active_calls: AtomicUsize,
    delay: Duration,
}

#[derive(Default)]
struct RecordingTelemetry {
    events: Mutex<Vec<MemoryEvent>>,
}

#[derive(Default)]
struct SessionRecordingHandle {
    sessions: Mutex<Vec<Option<coco_types::SessionId>>>,
}

impl crate::telemetry::MemoryTelemetryEmitter for RecordingTelemetry {
    fn emit(&self, event: MemoryEvent) {
        self.events
            .lock()
            .expect("telemetry events lock")
            .push(event);
    }
}

impl RecordingTelemetry {
    fn events(&self) -> Vec<MemoryEvent> {
        self.events.lock().expect("telemetry events lock").clone()
    }
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

impl SessionRecordingHandle {
    fn sessions(&self) -> Vec<Option<coco_types::SessionId>> {
        self.sessions
            .lock()
            .expect("recorded sessions lock")
            .clone()
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
        _session_id: &coco_types::SessionId,
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

#[async_trait::async_trait]
impl AgentHandle for SessionRecordingHandle {
    async fn spawn_agent(&self, request: AgentSpawnRequest) -> Result<AgentSpawnResponse, String> {
        self.sessions
            .lock()
            .expect("recorded sessions lock")
            .push(request.session_id);
        Ok(AgentSpawnResponse {
            status: AgentSpawnStatus::Completed,
            agent_id: Some("memory".into()),
            result: Some("ok".into()),
            total_tool_use_count: 1,
            duration_ms: 1,
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
        _session_id: &coco_types::SessionId,
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

fn patch_overrides() -> Arc<ToolOverrides> {
    Arc::new(
        ToolOverrides::none()
            .with_extra(ToolId::Builtin(ToolName::ApplyPatch))
            .with_excluded(ToolId::Builtin(ToolName::Write))
            .with_excluded(ToolId::Builtin(ToolName::Edit)),
    )
}

#[test]
fn escape_memory_close_tags_handles_multibyte_prefix() {
    assert_eq!(
        escape_memory_close_tags("备注</MeMoRy> should escape"),
        "备注&lt;/memory> should escape"
    );
}

#[test]
fn prompt_index_segments_reject_runtime_escape_paths() {
    assert!(prompt_index_segments_are_safe("index/MEMORY.md"));
    assert!(!prompt_index_segments_are_safe(""));
    assert!(!prompt_index_segments_are_safe("../escape.md"));
    assert!(!prompt_index_segments_are_safe("a/../b.md"));
    assert!(!prompt_index_segments_are_safe("a//b.md"));
    assert!(!prompt_index_segments_are_safe("space here.md"));
}

#[tokio::test]
async fn render_system_prompt_section_includes_mounted_prompt_index() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let store_dir = tmp.path().join("store");
    let index_file = store_dir.join("index").join("MEMORY.md");
    tokio::fs::create_dir_all(index_file.parent().expect("parent"))
        .await
        .expect("create index parent");
    tokio::fs::write(
        &index_file,
        "- [One](one.md) - hook\n</MeMoRy> should escape",
    )
    .await
    .expect("write prompt index");

    let stores_raw = serde_json::json!([
        {
            "path": store_dir.display().to_string(),
            "mount": "shared",
            "promptIndex": "index/MEMORY.md"
        }
    ])
    .to_string();
    let mut config = MemoryConfig {
        dream_enabled: false,
        session_memory_enabled: false,
        memory_stores: coco_config::try_parse_memory_stores(&stores_raw).expect("stores"),
        ..Default::default()
    };
    config.directory = Some(tmp.path().join("memory"));
    let runtime = MemoryRuntimeBuilder::new(
        tmp.path().join("home"),
        tmp.path().join("project"),
        "session-1",
        config,
        Arc::new(SlowHandle::default()),
    )
    .build();

    let prompt = runtime
        .render_system_prompt_section()
        .await
        .expect("prompt");

    assert!(prompt.contains("The following is the memory index at `team/shared/index/MEMORY.md`"));
    assert!(prompt.contains("<memory path=\"team/shared/index/MEMORY.md\">"));
    assert!(prompt.contains("- [One](one.md) - hook"));
    assert!(prompt.contains("&lt;/memory> should escape"));
}

#[tokio::test]
async fn render_system_prompt_section_uses_runtime_tool_overrides() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let agent: AgentHandleRef = Arc::new(coco_tool_runtime::NoOpAgentHandle);
    let mut config = MemoryConfig {
        dream_enabled: false,
        session_memory_enabled: false,
        ..Default::default()
    };
    config.directory = Some(tmp.path().join("memory"));

    let native = MemoryRuntimeBuilder::new(
        tmp.path().join("home"),
        tmp.path().join("project"),
        "s1",
        config.clone(),
        agent.clone(),
    )
    .build()
    .render_system_prompt_section()
    .await
    .expect("native prompt");
    assert!(native.contains("Write tool"));
    assert!(!native.contains("apply_patch"));

    let patch = MemoryRuntimeBuilder::new(
        tmp.path().join("home2"),
        tmp.path().join("project"),
        "s2",
        config,
        agent,
    )
    .with_tool_overrides(patch_overrides())
    .build()
    .render_system_prompt_section()
    .await
    .expect("patch prompt");
    assert!(patch.contains("apply_patch"));
    assert!(!patch.contains("Write"));
    assert!(!patch.contains("Edit"));
}

#[tokio::test]
async fn render_system_prompt_section_full_guidelines_override_skips_standard_memory_prompt() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let memory_dir = tmp.path().join("memory");
    tokio::fs::create_dir_all(&memory_dir)
        .await
        .expect("create memory dir");
    tokio::fs::write(
        memory_dir.join("MEMORY.md"),
        "- [Private](private.md) - should not render",
    )
    .await
    .expect("write private index");

    let store_dir = tmp.path().join("store");
    tokio::fs::create_dir_all(&store_dir)
        .await
        .expect("create store");
    tokio::fs::write(
        store_dir.join("MEMORY.md"),
        "- [Mounted](mounted.md) - hook",
    )
    .await
    .expect("write mounted index");

    let stores_raw = serde_json::json!([
        {
            "path": store_dir.display().to_string(),
            "mount": "shared",
            "promptIndex": "MEMORY.md"
        }
    ])
    .to_string();
    let config = MemoryConfig {
        directory: Some(memory_dir),
        dream_enabled: false,
        session_memory_enabled: false,
        guidelines: Some("  custom memory policy  ".to_string()),
        extra_guidelines: Some("extra policy should not render".to_string()),
        memory_stores: coco_config::try_parse_memory_stores(&stores_raw).expect("stores"),
        ..Default::default()
    };
    let telemetry = Arc::new(RecordingTelemetry::default());
    let runtime = MemoryRuntimeBuilder::new(
        tmp.path().join("home"),
        tmp.path().join("project"),
        "session-1",
        config,
        Arc::new(SlowHandle::default()),
    )
    .with_telemetry(telemetry.clone())
    .build();

    let prompt = runtime
        .render_system_prompt_section()
        .await
        .expect("prompt");

    assert_eq!(prompt, "# auto memory\ncustom memory policy");
    assert!(telemetry.events().is_empty());
}

#[tokio::test]
async fn render_system_prompt_section_caps_full_guidelines_override() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let config = MemoryConfig {
        directory: Some(tmp.path().join("memory")),
        dream_enabled: false,
        session_memory_enabled: false,
        guidelines: Some("火".repeat(20_000)),
        ..Default::default()
    };
    let runtime = MemoryRuntimeBuilder::new(
        tmp.path().join("home"),
        tmp.path().join("project"),
        "session-1",
        config,
        Arc::new(SlowHandle::default()),
    )
    .build();

    let prompt = runtime
        .render_system_prompt_section()
        .await
        .expect("prompt");

    assert!(prompt.len() <= "# auto memory\n".len() + 25 * 1024);
    assert!(prompt.contains("omitted"));
    assert!(prompt.is_char_boundary(prompt.len()));
}

#[tokio::test]
async fn render_system_prompt_section_caps_extra_guidelines_and_mounted_indexes() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let store_dir = tmp.path().join("store");
    tokio::fs::create_dir_all(&store_dir)
        .await
        .expect("create store");
    tokio::fs::write(store_dir.join("MEMORY.md"), "mounted\n".repeat(10_000))
        .await
        .expect("write mounted index");

    let stores_raw = serde_json::json!([
        {
            "path": store_dir.display().to_string(),
            "mount": "shared",
            "promptIndex": "MEMORY.md"
        }
    ])
    .to_string();
    let config = MemoryConfig {
        directory: Some(tmp.path().join("memory")),
        dream_enabled: false,
        session_memory_enabled: false,
        extra_guidelines: Some("extra\n".repeat(10_000)),
        memory_stores: coco_config::try_parse_memory_stores(&stores_raw).expect("stores"),
        ..Default::default()
    };
    let runtime = MemoryRuntimeBuilder::new(
        tmp.path().join("home"),
        tmp.path().join("project"),
        "session-1",
        config,
        Arc::new(SlowHandle::default()),
    )
    .build();

    let prompt = runtime
        .render_system_prompt_section()
        .await
        .expect("prompt");

    assert!(prompt.contains("omitted"));
    assert!(prompt.len() < 40 * 1024);
}

#[tokio::test]
async fn render_system_prompt_section_uses_team_only_for_mounted_team_stores_without_user_store() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let memory_dir = tmp.path().join("memory");
    let personal_dir = memory_dir.clone();
    let team_dir = memory_dir.join("team");
    tokio::fs::create_dir_all(&team_dir)
        .await
        .expect("create team dir");
    tokio::fs::write(
        personal_dir.join("MEMORY.md"),
        "- [Private](private.md) - should not render",
    )
    .await
    .expect("write private index");
    tokio::fs::write(
        team_dir.join("MEMORY.md"),
        "- [Team root](team.md) - should not render",
    )
    .await
    .expect("write team root index");

    let store_dir = tmp.path().join("store");
    let index_file = store_dir.join("MEMORY.md");
    tokio::fs::create_dir_all(&store_dir)
        .await
        .expect("create store");
    tokio::fs::write(&index_file, "- [Mounted](mounted.md) - hook")
        .await
        .expect("write mounted index");

    let stores_raw = serde_json::json!([
        {
            "path": store_dir.display().to_string(),
            "mount": "shared",
            "promptIndex": "MEMORY.md"
        }
    ])
    .to_string();
    let config = MemoryConfig {
        directory: Some(memory_dir),
        dream_enabled: false,
        session_memory_enabled: false,
        memory_stores: coco_config::try_parse_memory_stores(&stores_raw).expect("stores"),
        ..Default::default()
    };
    let runtime = MemoryRuntimeBuilder::new(
        tmp.path().join("home"),
        tmp.path().join("project"),
        "session-1",
        config,
        Arc::new(SlowHandle::default()),
    )
    .build();

    let prompt = runtime
        .render_system_prompt_section()
        .await
        .expect("prompt");

    assert!(prompt.starts_with("# Memory"));
    assert!(prompt.contains("persistent, file-based team memory directory"));
    assert!(prompt.contains("There is no separate private memory directory"));
    assert!(prompt.contains("team/shared/MEMORY.md"));
    assert!(prompt.contains("- [Mounted](mounted.md) - hook"));
    let mount_path = team_dir.join("shared");
    assert_eq!(
        std::fs::read_link(&mount_path).expect("mounted store symlink"),
        store_dir
    );
    assert!(!prompt.contains("private directory at"));
    assert!(!prompt.contains("## MEMORY.md"));
    assert!(!prompt.contains("Team MEMORY.md"));
    assert!(!prompt.contains("should not render"));
}

#[tokio::test]
async fn render_system_prompt_section_mentions_empty_read_only_mounted_prompt_index() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let store_dir = tmp.path().join("readonly-store");
    tokio::fs::create_dir_all(&store_dir)
        .await
        .expect("create read-only store");
    let stores_raw = serde_json::json!([
        {
            "path": store_dir.display().to_string(),
            "mode": "ro",
            "mount": "readonly",
            "promptIndex": "MEMORY.md"
        }
    ])
    .to_string();
    let mut config = MemoryConfig {
        dream_enabled: false,
        session_memory_enabled: false,
        memory_stores: coco_config::try_parse_memory_stores(&stores_raw).expect("stores"),
        ..Default::default()
    };
    config.directory = Some(tmp.path().join("memory"));
    let runtime = MemoryRuntimeBuilder::new(
        tmp.path().join("home"),
        tmp.path().join("project"),
        "session-1",
        config,
        Arc::new(SlowHandle::default()),
    )
    .build();

    let prompt = runtime
        .render_system_prompt_section()
        .await
        .expect("prompt");

    assert!(prompt.contains(
        "You have a read-only team memory index at `team/readonly/MEMORY.md` (currently empty)."
    ));
}

#[tokio::test]
async fn mounted_prompt_index_emits_ok_for_existing_and_missing_files() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let existing_store = tmp.path().join("existing-store");
    let existing_index = existing_store.join("MEMORY.md");
    tokio::fs::create_dir_all(&existing_store)
        .await
        .expect("create existing store");
    tokio::fs::write(&existing_index, "- [One](one.md) - hook")
        .await
        .expect("write prompt index");

    let missing_store = tmp.path().join("missing-store");
    tokio::fs::create_dir_all(&missing_store)
        .await
        .expect("create missing-index store");
    let stores_raw = serde_json::json!([
        {
            "path": existing_store.display().to_string(),
            "mount": "existing",
            "promptIndex": "MEMORY.md"
        },
        {
            "path": missing_store.display().to_string(),
            "mount": "missing",
            "promptIndex": "MEMORY.md"
        }
    ])
    .to_string();
    let mut config = MemoryConfig {
        dream_enabled: false,
        session_memory_enabled: false,
        memory_stores: coco_config::try_parse_memory_stores(&stores_raw).expect("stores"),
        ..Default::default()
    };
    config.directory = Some(tmp.path().join("memory"));
    let telemetry = Arc::new(RecordingTelemetry::default());
    let runtime = MemoryRuntimeBuilder::new(
        tmp.path().join("home"),
        tmp.path().join("project"),
        "session-1",
        config,
        Arc::new(SlowHandle::default()),
    )
    .with_telemetry(telemetry.clone())
    .build();

    let prompt = runtime
        .render_system_prompt_section()
        .await
        .expect("prompt");

    assert!(prompt.contains("team/existing/MEMORY.md"));
    assert!(prompt.contains("team/missing/MEMORY.md"));
    let prompt_index_events = telemetry
        .events()
        .into_iter()
        .filter_map(|event| match event {
            MemoryEvent::MemoryPromptIndex {
                mount,
                prompt_index,
                outcome,
            } => Some((mount, prompt_index, outcome)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        prompt_index_events,
        vec![
            (
                "existing".to_string(),
                "MEMORY.md".to_string(),
                MemoryPromptIndexOutcome::Ok,
            ),
            (
                "missing".to_string(),
                "MEMORY.md".to_string(),
                MemoryPromptIndexOutcome::Ok,
            ),
        ]
    );
}

#[tokio::test]
async fn render_system_prompt_section_omits_unmaterialized_writable_mounted_store() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let missing_store = tmp.path().join("missing-store");
    let stores_raw = serde_json::json!([
        {
            "path": missing_store.display().to_string(),
            "mount": "missing",
            "promptIndex": "MEMORY.md"
        }
    ])
    .to_string();
    let config = MemoryConfig {
        directory: Some(tmp.path().join("memory")),
        dream_enabled: false,
        session_memory_enabled: false,
        memory_stores: coco_config::try_parse_memory_stores(&stores_raw).expect("stores"),
        ..Default::default()
    };
    let runtime = MemoryRuntimeBuilder::new(
        tmp.path().join("home"),
        tmp.path().join("project"),
        "session-1",
        config,
        Arc::new(SlowHandle::default()),
    )
    .build();

    let prompt = runtime
        .render_system_prompt_section()
        .await
        .expect("prompt");

    assert!(!prompt.contains("team/missing/"));
    assert!(prompt.contains("read-only access to team memory"));
}

#[tokio::test]
async fn mounted_prompt_index_emits_error_for_unreadable_file() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let store_dir = tmp.path().join("store");
    let index_path = store_dir.join("MEMORY.md");
    tokio::fs::create_dir_all(&index_path)
        .await
        .expect("create directory at prompt index path");

    let stores_raw = serde_json::json!([
        {
            "path": store_dir.display().to_string(),
            "mount": "shared",
            "promptIndex": "MEMORY.md"
        }
    ])
    .to_string();
    let mut config = MemoryConfig {
        dream_enabled: false,
        session_memory_enabled: false,
        memory_stores: coco_config::try_parse_memory_stores(&stores_raw).expect("stores"),
        ..Default::default()
    };
    config.directory = Some(tmp.path().join("memory"));
    let telemetry = Arc::new(RecordingTelemetry::default());
    let runtime = MemoryRuntimeBuilder::new(
        tmp.path().join("home"),
        tmp.path().join("project"),
        "session-1",
        config,
        Arc::new(SlowHandle::default()),
    )
    .with_telemetry(telemetry.clone())
    .build();

    runtime
        .render_system_prompt_section()
        .await
        .expect("prompt");

    let prompt_index_events = telemetry
        .events()
        .into_iter()
        .filter_map(|event| match event {
            MemoryEvent::MemoryPromptIndex {
                mount,
                prompt_index,
                outcome,
            } => Some((mount, prompt_index, outcome)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        prompt_index_events,
        vec![(
            "shared".to_string(),
            "MEMORY.md".to_string(),
            MemoryPromptIndexOutcome::Error,
        )]
    );
}

#[tokio::test]
async fn mounted_prompt_index_emits_unsafe_path_when_store_bypasses_parser() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let store_dir = tmp.path().join("store");
    tokio::fs::create_dir_all(&store_dir)
        .await
        .expect("create store");
    let stores_raw = serde_json::json!([
        {
            "path": store_dir.display().to_string(),
            "mount": "shared",
            "promptIndex": "MEMORY.md"
        }
    ])
    .to_string();
    let mut stores = coco_config::try_parse_memory_stores(&stores_raw).expect("stores");
    stores[0].prompt_index = Some("../escape.md".to_string());
    let mut config = MemoryConfig {
        dream_enabled: false,
        session_memory_enabled: false,
        memory_stores: stores,
        ..Default::default()
    };
    config.directory = Some(tmp.path().join("memory"));
    let telemetry = Arc::new(RecordingTelemetry::default());
    let runtime = MemoryRuntimeBuilder::new(
        tmp.path().join("home"),
        tmp.path().join("project"),
        "session-1",
        config,
        Arc::new(SlowHandle::default()),
    )
    .with_telemetry(telemetry.clone())
    .build();

    runtime
        .render_system_prompt_section()
        .await
        .expect("prompt");

    let prompt_index_events = telemetry
        .events()
        .into_iter()
        .filter_map(|event| match event {
            MemoryEvent::MemoryPromptIndex {
                mount,
                prompt_index,
                outcome,
            } => Some((mount, prompt_index, outcome)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        prompt_index_events,
        vec![(
            "shared".to_string(),
            "../escape.md".to_string(),
            MemoryPromptIndexOutcome::UnsafePath,
        )]
    );
}

#[tokio::test]
async fn finalize_turn_warns_when_entrypoint_index_approaches_line_cap() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let mut config = MemoryConfig {
        dream_enabled: false,
        extraction_enabled: false,
        session_memory_enabled: false,
        ..Default::default()
    };
    config.directory = Some(tmp.path().join("memory"));
    let runtime = MemoryRuntimeBuilder::new(
        tmp.path().join("home"),
        tmp.path().join("project"),
        "session-1",
        config,
        Arc::new(SlowHandle::default()),
    )
    .build();
    tokio::fs::create_dir_all(runtime.personal_dir())
        .await
        .expect("create memory dir");
    let index_path = runtime.personal_dir().join("MEMORY.md");
    let lines = (0..160)
        .map(|i| format!("- [Item {i}](item_{i}.md) - hook"))
        .collect::<Vec<_>>()
        .join("\n");
    tokio::fs::write(&index_path, lines)
        .await
        .expect("write index");

    let mut ctx = finalize_ctx("msg-1");
    ctx.recent_tool_writes = vec![ToolWriteRecord {
        tool_name: "Edit".into(),
        file_path: index_path,
        succeeded: true,
    }];
    let report = runtime.finalize_turn(ctx).await;

    assert_eq!(report.index_warnings.len(), 1);
    assert!(report.index_warnings[0].contains(
        "The memory index at MEMORY.md is 160 lines, approaching the 200-line read limit."
    ));
    assert!(
        report.index_warnings[0].contains("Compact it to under 140 lines now"),
        "warning was: {}",
        report.index_warnings[0]
    );
}

#[tokio::test]
async fn finalize_turn_warns_when_mounted_prompt_index_approaches_byte_cap() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let memory_dir = tmp.path().join("memory");
    let store_dir = memory_dir.join("team").join("shared");
    let stores_raw = serde_json::json!([
        {
            "path": store_dir.display().to_string(),
            "mount": "shared",
            "promptIndex": "MEMORY.md",
            "promptIndexMaxBytes": 100
        }
    ])
    .to_string();
    let mut config = MemoryConfig {
        dream_enabled: false,
        extraction_enabled: false,
        session_memory_enabled: false,
        memory_stores: coco_config::try_parse_memory_stores(&stores_raw).expect("stores"),
        ..Default::default()
    };
    config.directory = Some(memory_dir);
    let runtime = MemoryRuntimeBuilder::new(
        tmp.path().join("home"),
        tmp.path().join("project"),
        "session-1",
        config,
        Arc::new(SlowHandle::default()),
    )
    .build();
    tokio::fs::create_dir_all(&store_dir)
        .await
        .expect("create mounted store");
    let index_path = store_dir.join("MEMORY.md");
    tokio::fs::write(&index_path, "x".repeat(85))
        .await
        .expect("write prompt index");

    let mut ctx = finalize_ctx("msg-1");
    ctx.recent_tool_writes = vec![ToolWriteRecord {
        tool_name: "Write".into(),
        file_path: index_path,
        succeeded: true,
    }];
    let report = runtime.finalize_turn(ctx).await;

    assert_eq!(report.index_warnings.len(), 1);
    assert!(report.index_warnings[0].contains(
        "The memory index at team/shared/MEMORY.md is 85 bytes, approaching the 100 bytes read limit."
    ));
    assert!(
        report.index_warnings[0].contains("Compact it to under 70 bytes now"),
        "warning was: {}",
        report.index_warnings[0]
    );
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
async fn retargeted_memory_runtime_updates_extract_session_identity() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let agent = Arc::new(SessionRecordingHandle::default());
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
        "session-old",
        config,
        agent.clone(),
    )
    .build();

    runtime
        .set_session_id(coco_types::SessionId::try_new("session-new").expect("session id"))
        .await;

    let report = runtime.finalize_turn(finalize_ctx("msg-1")).await;

    assert!(!report.skipped);
    assert!(runtime.drain(Duration::from_secs(2)).await);
    assert_eq!(
        agent.sessions(),
        vec![Some(
            coco_types::SessionId::try_new("session-new").expect("session id")
        )]
    );
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
