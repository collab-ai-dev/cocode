use std::fs;
use std::sync::Arc;

use coco_inference::AISdkError;
use coco_inference::LanguageModel;
use coco_inference::LanguageModelCallOptions;
use coco_inference::LanguageModelGenerateResult;
use coco_inference::LanguageModelStreamResult;
use coco_llm_types::AssistantContentPart;
use coco_llm_types::FinishReason;
use coco_llm_types::StopReason;
use coco_llm_types::TextPart;
use coco_llm_types::Usage;
use coco_messages::Message;
use coco_messages::MessageHistory;
use coco_sandbox::EnforcementLevel;
use coco_sandbox::SandboxConfig;
use coco_sandbox::SandboxSettings;
use coco_sandbox::SandboxState;
use coco_tool_runtime::ToolRegistry;
use coco_tool_runtime::ToolUseContext;
use coco_types::AttachmentKind;
use pretty_assertions::assert_eq;
use tempfile::tempdir;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::config::QueryEngineConfig;
use crate::engine::QueryEngine;

/// Minimal mock — drain logic doesn't drive the model, but `QueryEngine`
/// requires a non-null client to construct.
struct StubModel;

#[async_trait::async_trait]
impl LanguageModel for StubModel {
    fn provider(&self) -> &str {
        "stub"
    }
    fn model_id(&self) -> &str {
        "stub"
    }
    async fn do_generate(
        &self,
        _options: &LanguageModelCallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelGenerateResult, AISdkError> {
        Ok(LanguageModelGenerateResult {
            content: vec![AssistantContentPart::Text(TextPart {
                text: "".into(),
                provider_metadata: None,
            })],
            usage: Usage::new(0, 0),
            finish_reason: FinishReason::new(StopReason::EndTurn),
            warnings: vec![],
            provider_metadata: None,
            request: None,
            response: None,
        })
    }
    async fn do_stream(
        &self,
        options: &LanguageModelCallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelStreamResult, AISdkError> {
        let result = self.do_generate(options, None).await?;
        Ok(coco_inference::synthetic_stream_from_content(
            result.content,
            result.usage,
            result.finish_reason,
        ))
    }
}

fn make_test_engine() -> QueryEngine {
    let model = Arc::new(StubModel);
    let client = crate::test_support::model_runtime_registry(model);
    let tools = Arc::new(ToolRegistry::new());
    let cancel = CancellationToken::new();
    QueryEngine::new(
        QueryEngineConfig::default(),
        coco_types::SessionId::try_new("test-session").unwrap(),
        client,
        tools,
        cancel,
        None,
    )
}

fn make_test_ctx_with_cwd(cwd: std::path::PathBuf) -> ToolUseContext {
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(cwd);
    ctx
}

fn make_sandbox_state(config: SandboxConfig) -> Arc<SandboxState> {
    Arc::new(SandboxState::new(
        config.enforcement,
        SandboxSettings::enabled(),
        config,
        coco_sandbox::platform::create_platform(),
    ))
}

#[tokio::test]
async fn drain_empty_set_is_noop() {
    let engine = make_test_engine();
    let dir = tempdir().unwrap();
    let ctx = make_test_ctx_with_cwd(dir.path().to_path_buf());

    engine.drain_nested_memory_triggers(&ctx).await;
    let pending = engine.take_pending_nested_memory().await;
    assert!(
        pending.is_empty(),
        "empty trigger Set must produce no pending entries"
    );
}

#[tokio::test]
async fn drain_changed_files_injects_edited_text_file_message_once() {
    let root = tempdir().unwrap();
    let file = root.path().join("watched.rs");
    fs::write(&file, "fn main() {}\n").unwrap();
    let mtime = coco_context::file_mtime_ms(&file).await.unwrap();

    let frs = Arc::new(RwLock::new(coco_context::FileReadState::new()));
    frs.write().await.set(
        file.clone(),
        coco_context::FileReadEntry::full_real("fn main() {}\n".to_string(), mtime),
    );

    std::thread::sleep(std::time::Duration::from_millis(50));
    fs::write(&file, "fn main() {\n    println!(\"changed\");\n}\n").unwrap();

    let engine = make_test_engine().with_file_read_state(frs);
    let ctx = make_test_ctx_with_cwd(root.path().to_path_buf());
    let mut history = MessageHistory::default();
    engine.drain_changed_files(&ctx, &mut history, &None).await;

    let edited = history
        .iter()
        .filter_map(|msg| match msg.as_ref() {
            Message::Attachment(att) if att.kind == AttachmentKind::EditedTextFile => Some(att),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(edited.len(), 1);
    let text = edited[0].as_text_for_display();
    assert!(text.contains("was modified"));
    assert!(text.contains("println!(\"changed\")"));
    {
        let guard = engine.file_read_state.as_ref().unwrap().read().await;
        let entry = guard.peek(&file).unwrap();
        assert_eq!(entry.evidence, coco_context::ReadEvidence::ObservedForDiff);
        assert!(
            !entry.can_satisfy_edit_or_write(),
            "changed-file snippet must not satisfy edit/write evidence"
        );
    }

    engine.drain_changed_files(&ctx, &mut history, &None).await;
    let edited_count = history
        .iter()
        .filter(|msg| {
            matches!(
                msg.as_ref(),
                Message::Attachment(att) if att.kind == AttachmentKind::EditedTextFile
            )
        })
        .count();
    assert_eq!(
        edited_count, 1,
        "updated FileReadState must prevent duplicates"
    );
}

#[tokio::test]
async fn drain_changed_files_respects_read_ignore_patterns() {
    let root = tempdir().unwrap();
    let file = root.path().join("blocked.env");
    fs::write(&file, "TOKEN=old\n").unwrap();
    let mtime = coco_context::file_mtime_ms(&file).await.unwrap();

    let frs = Arc::new(RwLock::new(coco_context::FileReadState::new()));
    frs.write().await.set(
        file.clone(),
        coco_context::FileReadEntry::full_real("TOKEN=old\n".to_string(), mtime),
    );

    std::thread::sleep(std::time::Duration::from_millis(50));
    fs::write(&file, "TOKEN=new\n").unwrap();

    let engine = make_test_engine().with_file_read_state(frs);
    let mut ctx = make_test_ctx_with_cwd(root.path().to_path_buf());
    ctx.tool_config.file_read_ignore_patterns = vec!["*.env".to_string()];
    let mut history = MessageHistory::default();

    engine.drain_changed_files(&ctx, &mut history, &None).await;

    assert!(
        history.iter().all(|msg| {
            !matches!(
                msg.as_ref(),
                Message::Attachment(att) if att.kind == AttachmentKind::EditedTextFile
            )
        }),
        "read-denied changed files must not be surfaced"
    );
    let guard = engine.file_read_state.as_ref().unwrap().read().await;
    let entry = guard.peek(&file).unwrap();
    assert_eq!(
        entry.mtime_ms, mtime,
        "denied changed-file scans must not update the observed baseline"
    );
}

#[tokio::test]
async fn drain_changed_files_respects_sandbox_preflight() {
    let root = tempdir().unwrap();
    let file = root.path().join("blocked.txt");
    fs::write(&file, "old\n").unwrap();
    let mtime = coco_context::file_mtime_ms(&file).await.unwrap();

    let frs = Arc::new(RwLock::new(coco_context::FileReadState::new()));
    frs.write().await.set(
        file.clone(),
        coco_context::FileReadEntry::full_real("old\n".to_string(), mtime),
    );

    std::thread::sleep(std::time::Duration::from_millis(50));
    fs::write(&file, "new\n").unwrap();

    let engine = make_test_engine().with_file_read_state(frs);
    let mut ctx = make_test_ctx_with_cwd(root.path().to_path_buf());
    ctx.sandbox_state = Some(make_sandbox_state(SandboxConfig {
        enforcement: EnforcementLevel::WorkspaceWrite,
        denied_read_paths: vec![file.clone()],
        ..SandboxConfig::default()
    }));
    let mut history = MessageHistory::default();

    engine.drain_changed_files(&ctx, &mut history, &None).await;

    assert!(history.iter().all(|msg| {
        !matches!(
            msg.as_ref(),
            Message::Attachment(att) if att.kind == AttachmentKind::EditedTextFile
        )
    }));
    let guard = engine.file_read_state.as_ref().unwrap().read().await;
    let entry = guard.peek(&file).unwrap();
    assert_eq!(entry.mtime_ms, mtime);
    assert_eq!(entry.content, "old\n");
}

#[tokio::test]
async fn drain_changed_image_queues_generator_input_without_history_message() {
    let root = tempdir().unwrap();
    let file = root.path().join("screen.png");
    fs::write(&file, b"old").unwrap();
    let mtime = coco_context::file_mtime_ms(&file).await.unwrap();

    let frs = Arc::new(RwLock::new(coco_context::FileReadState::new()));
    frs.write().await.set(
        file.clone(),
        coco_context::FileReadEntry::full_real(String::new(), mtime),
    );

    std::thread::sleep(std::time::Duration::from_millis(50));
    fs::write(&file, b"new").unwrap();

    let engine = make_test_engine().with_file_read_state(frs);
    let ctx = make_test_ctx_with_cwd(root.path().to_path_buf());
    let mut history = MessageHistory::default();

    engine.drain_changed_files(&ctx, &mut history, &None).await;

    assert!(history.iter().all(|msg| {
        !matches!(
            msg.as_ref(),
            Message::Attachment(att) if att.kind == AttachmentKind::EditedImageFile
        )
    }));
    let pending = engine.take_pending_edited_image_file_paths().await;
    assert_eq!(pending, vec![file]);
}

#[tokio::test]
async fn drain_traverses_intermediate_claude_md() {
    // CWD = /tmp/proj. Trigger file = /tmp/proj/sub/handler.rs.
    // Should pick up /tmp/proj/sub/CLAUDE.md (descendant of CWD).
    let root = tempdir().unwrap();
    let proj = root.path().join("proj");
    let sub = proj.join("sub");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("CLAUDE.md"), "# sub").unwrap();
    let trigger = sub.join("handler.rs");
    fs::write(&trigger, "").unwrap();

    let engine = make_test_engine();
    let ctx = make_test_ctx_with_cwd(proj.clone());

    // Simulate a tool push.
    {
        let mut triggers = ctx.nested_memory_attachment_triggers.write().await;
        triggers.insert(trigger.canonicalize().unwrap().display().to_string());
    }

    engine.drain_nested_memory_triggers(&ctx).await;

    // Trigger Set is now empty.
    assert!(
        ctx.nested_memory_attachment_triggers
            .read()
            .await
            .is_empty(),
        "drain must clear the trigger Set in place"
    );

    let pending = engine.take_pending_nested_memory().await;
    assert_eq!(pending.len(), 1, "expected 1 entry, got {pending:?}");
    assert!(
        pending[0].path.contains("sub/CLAUDE.md") || pending[0].path.contains("sub\\CLAUDE.md"),
        "expected sub/CLAUDE.md path, got {}",
        pending[0].path
    );
    assert_eq!(pending[0].content, "# sub");
}

#[tokio::test]
async fn drain_records_transformed_memory_as_injected_raw_content() {
    let root = tempdir().unwrap();
    let proj = root.path().join("proj");
    let sub = proj.join("sub");
    fs::create_dir_all(&sub).unwrap();
    let memory = sub.join("CLAUDE.md");
    fs::write(&memory, "visible\n<!-- hidden -->\n").unwrap();
    let trigger = sub.join("handler.rs");
    fs::write(&trigger, "").unwrap();

    let frs = Arc::new(RwLock::new(coco_context::FileReadState::new()));
    let engine = make_test_engine().with_file_read_state(frs.clone());
    let ctx = make_test_ctx_with_cwd(proj.clone());
    ctx.nested_memory_attachment_triggers
        .write()
        .await
        .insert(trigger.canonicalize().unwrap().display().to_string());

    engine.drain_nested_memory_triggers(&ctx).await;

    let pending = engine.take_pending_nested_memory().await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].content, "visible\n\n");

    let frs_read = frs.read().await;
    let entry = frs_read
        .peek(&memory.canonicalize().unwrap())
        .expect("memory file should be recorded");
    assert_eq!(entry.content, "visible\n<!-- hidden -->\n");
    assert_eq!(entry.range, coco_context::FileReadRange::Full);
    assert_eq!(
        entry.evidence,
        coco_context::ReadEvidence::InjectedPartialView
    );
}

#[tokio::test]
async fn drain_records_untransformed_memory_as_real_full_read() {
    let root = tempdir().unwrap();
    let proj = root.path().join("proj");
    let sub = proj.join("sub");
    fs::create_dir_all(&sub).unwrap();
    let memory = sub.join("CLAUDE.md");
    fs::write(&memory, "visible\n").unwrap();
    let trigger = sub.join("handler.rs");
    fs::write(&trigger, "").unwrap();

    let frs = Arc::new(RwLock::new(coco_context::FileReadState::new()));
    let engine = make_test_engine().with_file_read_state(frs.clone());
    let ctx = make_test_ctx_with_cwd(proj.clone());
    ctx.nested_memory_attachment_triggers
        .write()
        .await
        .insert(trigger.canonicalize().unwrap().display().to_string());

    engine.drain_nested_memory_triggers(&ctx).await;

    let pending = engine.take_pending_nested_memory().await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].content, "visible\n");

    let frs_read = frs.read().await;
    let entry = frs_read
        .peek(&memory.canonicalize().unwrap())
        .expect("memory file should be recorded");
    assert_eq!(entry.content, "visible\n");
    assert_eq!(entry.range, coco_context::FileReadRange::Full);
    assert_eq!(entry.evidence, coco_context::ReadEvidence::RealFileView);
}

#[tokio::test]
async fn drain_dedupes_via_session_loaded_set() {
    // Two file reads under the same subtree should each surface
    // sub/CLAUDE.md — but only the first injection survives the
    // session-level dedup.
    let root = tempdir().unwrap();
    let proj = root.path().join("proj");
    let sub = proj.join("sub");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("CLAUDE.md"), "# sub").unwrap();
    let trigger1 = sub.join("a.rs");
    let trigger2 = sub.join("b.rs");
    fs::write(&trigger1, "").unwrap();
    fs::write(&trigger2, "").unwrap();

    let engine = make_test_engine();

    // Batch 1: trigger a.rs.
    {
        let ctx = make_test_ctx_with_cwd(proj.clone());
        ctx.nested_memory_attachment_triggers
            .write()
            .await
            .insert(trigger1.canonicalize().unwrap().display().to_string());
        engine.drain_nested_memory_triggers(&ctx).await;
    }
    let first = engine.take_pending_nested_memory().await;
    assert_eq!(first.len(), 1, "first batch should surface sub/CLAUDE.md");

    // Batch 2: trigger b.rs in the same subtree.
    {
        let ctx = make_test_ctx_with_cwd(proj.clone());
        ctx.nested_memory_attachment_triggers
            .write()
            .await
            .insert(trigger2.canonicalize().unwrap().display().to_string());
        engine.drain_nested_memory_triggers(&ctx).await;
    }
    let second = engine.take_pending_nested_memory().await;
    assert!(
        second.is_empty(),
        "second batch must not re-inject already-loaded sub/CLAUDE.md, got {second:?}"
    );

    // After clearing, third batch should re-inject.
    engine.clear_loaded_nested_memory_paths().await;
    {
        let ctx = make_test_ctx_with_cwd(proj.clone());
        ctx.nested_memory_attachment_triggers
            .write()
            .await
            .insert(trigger1.canonicalize().unwrap().display().to_string());
        engine.drain_nested_memory_triggers(&ctx).await;
    }
    let third = engine.take_pending_nested_memory().await;
    assert_eq!(
        third.len(),
        1,
        "after clear_loaded_nested_memory_paths, should re-inject"
    );
}

#[tokio::test]
async fn drain_dedupes_via_file_read_state() {
    // A CLAUDE.md already tracked in the session-persistent FileReadState
    // (a prior tool Read, or an injection from an earlier prompt cycle whose
    // per-cycle `loaded_nested_memory_paths` was since reset) must NOT be
    // re-injected. Mirrors the TS `readFileState.has()` gate in
    // `memoryFilesToAttachments` (the cross-cycle dedup the loaded-set alone
    // cannot provide, since the engine rebuilds it every user prompt).
    let root = tempdir().unwrap();
    let proj = root.path().join("proj");
    let sub = proj.join("sub");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("CLAUDE.md"), "# sub").unwrap();
    let trigger = sub.join("a.rs");
    fs::write(&trigger, "").unwrap();
    let trigger_key = trigger.canonicalize().unwrap().display().to_string();

    // Control: no FileReadState → CLAUDE.md injects. Capture its exact
    // emitted path so the gate test keys FileReadState identically (avoids
    // guessing how traversal canonicalizes the path).
    let injected_path = {
        let engine = make_test_engine();
        let ctx = make_test_ctx_with_cwd(proj.clone());
        ctx.nested_memory_attachment_triggers
            .write()
            .await
            .insert(trigger_key.clone());
        engine.drain_nested_memory_triggers(&ctx).await;
        let pending = engine.take_pending_nested_memory().await;
        assert_eq!(pending.len(), 1, "control: CLAUDE.md should inject");
        pending[0].path.clone()
    };

    // Gate: same trigger, but FileReadState already holds the CLAUDE.md.
    let frs = Arc::new(RwLock::new(coco_context::FileReadState::new()));
    frs.write().await.set(
        std::path::PathBuf::from(&injected_path),
        coco_context::FileReadEntry::full_real("# sub".into(), 0),
    );
    let engine = make_test_engine().with_file_read_state(frs);
    let ctx = make_test_ctx_with_cwd(proj.clone());
    ctx.nested_memory_attachment_triggers
        .write()
        .await
        .insert(trigger_key);

    engine.drain_nested_memory_triggers(&ctx).await;
    let pending = engine.take_pending_nested_memory().await;
    assert!(
        pending.is_empty(),
        "CLAUDE.md already in FileReadState must not be re-injected, got {pending:?}"
    );
}

#[tokio::test]
async fn drain_picks_up_agents_md() {
    // Runtime extension: AGENTS.md alongside CLAUDE.md.
    let root = tempdir().unwrap();
    let proj = root.path().join("proj");
    let sub = proj.join("sub");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("AGENTS.md"), "# agents").unwrap();
    let trigger = sub.join("f.rs");
    fs::write(&trigger, "").unwrap();

    let engine = make_test_engine();
    let ctx = make_test_ctx_with_cwd(proj.clone());
    ctx.nested_memory_attachment_triggers
        .write()
        .await
        .insert(trigger.canonicalize().unwrap().display().to_string());

    engine.drain_nested_memory_triggers(&ctx).await;
    let pending = engine.take_pending_nested_memory().await;
    assert_eq!(pending.len(), 1, "expected AGENTS.md to be picked up");
    assert!(pending[0].path.contains("AGENTS.md"));
}

#[tokio::test]
async fn drain_file_outside_cwd_emits_nothing() {
    // File outside CWD → nested_dirs empty → nothing to load.
    // (Phase 1 + Phase 4 conditional rules are still stubbed.)
    let root = tempdir().unwrap();
    let proj = root.path().join("proj");
    let elsewhere = root.path().join("other");
    fs::create_dir_all(&proj).unwrap();
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(elsewhere.join("CLAUDE.md"), "x").unwrap();
    let trigger = elsewhere.join("file.rs");
    fs::write(&trigger, "").unwrap();

    let engine = make_test_engine();
    let ctx = make_test_ctx_with_cwd(proj.clone());
    ctx.nested_memory_attachment_triggers
        .write()
        .await
        .insert(trigger.canonicalize().unwrap().display().to_string());

    engine.drain_nested_memory_triggers(&ctx).await;
    let pending = engine.take_pending_nested_memory().await;
    assert!(
        pending.is_empty(),
        "outside-CWD trigger must not surface intermediate CLAUDE.md, got {pending:?}"
    );
}

#[tokio::test]
async fn pending_slot_is_drained_after_take() {
    let engine = make_test_engine();
    {
        let mut p = engine.pending_nested_memory.lock().await;
        p.push(coco_system_reminder::generators::memory::NestedMemoryInfo {
            path: "/x".into(),
            content: "y".into(),
        });
    }
    let first = engine.take_pending_nested_memory().await;
    assert_eq!(first.len(), 1);

    let second = engine.take_pending_nested_memory().await;
    assert!(second.is_empty(), "take must clear the slot");
}
