//! End-to-end coverage for mid-turn steering (TS `messageQueueManager`).
//!
//! Pins the full chain:
//!
//! 1. A producer outside the engine clones [`coco_query::CommandQueue`]
//!    and enqueues a [`coco_query::QueuedCommand`] **while the engine is
//!    mid-turn** (the mock LLM is parked inside an artificial delay so
//!    we can race against the turn).
//! 2. The engine drains the queue at end-of-turn into history as a RAW
//!    `Message::User` tagged `MessageOrigin::QueuedSteering` (mirrors TS:
//!    the steered message IS a user message, stored as the user's plain
//!    text — no `<system-reminder>`, no framing — so it is fully visible in
//!    the transcript and counted as a user message).
//! 3. The next turn's API call sees the message **wrapped** in its prompt —
//!    `<system-reminder>` + the "The user sent a new message while you were
//!    working:" framing (TS `wrapCommandText`) — proving the wrapper is an
//!    API-serialization concern applied at prompt-build, not stored, and
//!    that the content actually reaches the model (the headline UX promise).
//! 4. Lifecycle events (`CommandDequeued{id}` per item +
//!    `QueueStateChanged{queued: 0}` summary) flow through `event_tx`
//!    so the TUI / SDK consumer can update its display.
//!
//! TS reference: `utils/messageQueueManager.ts` (the queue) +
//! `query.ts:1547-1643` (the snapshot/yield/dequeue dance) +
//! `messages.ts:3739` (`normalizeAttachmentForAPI`'s `queued_command`
//! case applies the system-reminder wrap).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicI32;
use std::sync::atomic::Ordering;
use std::time::Duration;

use coco_inference::AISdkError;
use coco_inference::LanguageModel;
use coco_inference::LanguageModelCallOptions;
use coco_inference::LanguageModelGenerateResult;
use coco_inference::LanguageModelStreamResult;
use coco_llm_types::AssistantContentPart;
use coco_llm_types::FinishReason;
use coco_llm_types::LlmMessage;
use coco_llm_types::StopReason;
use coco_llm_types::TextPart;
use coco_llm_types::ToolCallPart;
use coco_llm_types::Usage;
use coco_query::CommandQueue;
use coco_query::CoreEvent;
use coco_query::QueryEngine;
use coco_query::QueryEngineConfig;
use coco_query::QueuePriority;
use coco_query::QueuedCommand;
use coco_query::ServerNotification;
use coco_system_reminder::QueueOrigin;
use coco_tool_runtime::ToolRegistry;
use coco_types::AttachmentKind;
use coco_types::PermissionMode;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const STEERING_MARKER: &str = "STEER-XYZ-7117";

/// Two-call mock that sleeps in the first call so a producer task can
/// race a steering enqueue into the queue, then returns text in the
/// second call. Captures every `LanguageModelCallOptions` it sees so
/// the test can assert the queued content reached the second prompt.
struct SteeringMock {
    call_count: AtomicI32,
    captured_prompts: Arc<Mutex<Vec<Vec<LlmMessage>>>>,
    /// Time spent inside `do_generate` for the first call. Long enough
    /// for the producer task to wake and enqueue before the engine
    /// commits the assistant response and runs end-of-turn drain.
    first_call_delay: Duration,
}

impl SteeringMock {
    fn new(captured: Arc<Mutex<Vec<Vec<LlmMessage>>>>) -> Self {
        Self {
            call_count: AtomicI32::new(0),
            captured_prompts: captured,
            first_call_delay: Duration::from_millis(150),
        }
    }
}

#[async_trait::async_trait]
impl LanguageModel for SteeringMock {
    fn provider(&self) -> &str {
        "mock"
    }
    fn model_id(&self) -> &str {
        "steering-mock"
    }

    async fn do_generate(
        &self,
        options: &LanguageModelCallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelGenerateResult, AISdkError> {
        let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
        // Snapshot the prompt for the assertion phase. The guard is
        // dropped before any `.await` below — clippy's
        // `await_holding_lock` lints (correctly) against keeping a
        // sync `MutexGuard` live across suspension points.
        {
            let mut guard = self.captured_prompts.lock().unwrap();
            guard.push(options.prompt.clone());
        }

        if idx == 0 {
            // Park inside the call long enough for the producer task
            // (spawned by the test) to call `queue.enqueue(...)`. The
            // engine has nothing else to do during this delay, so on
            // `do_generate` return the assistant response is committed
            // and `finalize_turn_post_tools` runs the drain.
            tokio::time::sleep(self.first_call_delay).await;
            // Force a second turn by emitting a no-op tool call. Pick
            // a tool the mock harness's registry registers (we wire in
            // `BashTool` below) and feed it a benign command so it
            // succeeds and yields back to the loop. The actual tool
            // result content is unimportant — what matters is that the
            // engine takes a second turn after the drain runs, so the
            // captured prompt for call #1 includes the queued item.
            Ok(LanguageModelGenerateResult {
                content: vec![
                    AssistantContentPart::Text(TextPart {
                        text: "Working on it…".into(),
                        provider_metadata: None,
                    }),
                    AssistantContentPart::ToolCall(ToolCallPart {
                        tool_call_id: "steer_call_0".into(),
                        tool_name: "Bash".into(),
                        input: serde_json::json!({
                            "command": "true",
                            "description": "no-op marker"
                        }),
                        provider_executed: None,
                        provider_metadata: None,
                        invalid: false,
                        invalid_reason: None,
                    }),
                ],
                usage: Usage::new(50, 20),
                finish_reason: FinishReason::new(StopReason::ToolUse),
                warnings: vec![],
                provider_metadata: None,
                request: None,
                response: None,
            })
        } else {
            // Turn 2: end the conversation with a text reply that
            // explicitly echoes the marker, proving the model actually
            // saw the queued steering content.
            Ok(LanguageModelGenerateResult {
                content: vec![AssistantContentPart::Text(TextPart {
                    text: format!("Acknowledged: {STEERING_MARKER}"),
                    provider_metadata: None,
                })],
                usage: Usage::new(40, 15),
                finish_reason: FinishReason::new(StopReason::EndTurn),
                warnings: vec![],
                provider_metadata: None,
                request: None,
                response: None,
            })
        }
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

/// Build a registry with just `Bash` — that's the only tool the mock
/// invokes, and keeping the surface narrow keeps the test fast.
fn bash_only_tools() -> Arc<ToolRegistry> {
    let registry = ToolRegistry::new();
    registry.register(Arc::new(coco_tools::BashTool));
    Arc::new(registry)
}

/// Walk an `LlmMessage` of any role and return concatenated text from
/// every text-typed content part the model would see. Spans `User`,
/// `Tool`, `Assistant`, `System`, and `Developer` because TS' message
/// normalization (`smoosh_system_reminder_into_tool_result`) can fold
/// our queued-command attachment into the preceding `Tool` message —
/// only walking `User` would miss the steering content there.
fn extract_all_text(msg: &LlmMessage) -> String {
    use coco_llm_types::AssistantContentPart;
    use coco_llm_types::ToolContentPart;
    use coco_llm_types::ToolResultContent;
    use coco_llm_types::UserContentPart;
    let mut out = String::new();
    let mut push = |s: &str| {
        out.push_str(s);
        out.push('\n');
    };
    match msg {
        LlmMessage::User { content, .. } => {
            for p in content {
                if let UserContentPart::Text(t) = p {
                    push(&t.text);
                }
            }
        }
        LlmMessage::Assistant { content, .. } => {
            for p in content {
                if let AssistantContentPart::Text(t) = p {
                    push(&t.text);
                }
            }
        }
        LlmMessage::Tool { content, .. } => {
            for p in content {
                if let ToolContentPart::ToolResult(r) = p {
                    match &r.output {
                        ToolResultContent::Text { value, .. } => push(value),
                        ToolResultContent::Content { value, .. } => {
                            for c in value {
                                if let coco_llm_types::ToolResultContentPart::Text {
                                    text, ..
                                } = c
                                {
                                    push(text);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        LlmMessage::System { content, .. } | LlmMessage::Developer { content, .. } => {
            for p in content {
                if let UserContentPart::Text(t) = p {
                    push(&t.text);
                }
            }
        }
    }
    out
}

#[tokio::test]
async fn e2e_steering_drains_into_history_and_reaches_next_turn() {
    let captured: Arc<Mutex<Vec<Vec<LlmMessage>>>> = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(SteeringMock::new(captured.clone()));
    let client = coco_query::test_support::model_runtime_registry(model);
    let cancel = CancellationToken::new();
    let config = QueryEngineConfig {
        model_id: "steering-mock".into(),
        permission_mode: PermissionMode::BypassPermissions,
        // 4 turns is plenty: turn 1 queues bash, turn 2 returns text.
        max_turns: Some(4),
        ..Default::default()
    };

    // Hoist a session-scoped queue identical to what `SessionRuntime`
    // does in production (see `wire_engine`). The producer task and
    // the engine share this same `Arc`-backed handle.
    let queue = CommandQueue::new();

    let engine = QueryEngine::new(config, client, bash_only_tools(), cancel, None)
        .with_command_queue(queue.clone());

    // Producer task: wait long enough for the engine to enter
    // `do_generate` (mock parks for 150 ms there) and inject the
    // steering message. `Now` priority isn't strictly required — the
    // end-of-turn drain takes everything up to `Later` — but using it
    // exercises the highest-priority path.
    let producer_queue = queue.clone();
    let producer = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let item = QueuedCommand::new(
            format!("Add the literal token {STEERING_MARKER} to your reply."),
            QueuePriority::Now,
        )
        .with_origin(QueueOrigin::Human);
        producer_queue.enqueue(item).await;
    });

    // Capture every `CoreEvent` the engine emits so the assertion
    // phase can verify the queue lifecycle events round-trip.
    let (event_tx, mut event_rx) = mpsc::channel::<CoreEvent>(64);
    let event_collector = tokio::spawn(async move {
        let mut events = Vec::new();
        while let Some(e) = event_rx.recv().await {
            events.push(e);
        }
        events
    });

    let initial_messages = vec![std::sync::Arc::new(coco_messages::create_user_message(
        "kick off a turn so we can steer it",
    ))];
    let result = engine
        .run_with_messages(initial_messages, event_tx, coco_types::TurnId::generate())
        .await
        .expect("engine should complete");
    let _ = producer.await;
    let events = event_collector.await.expect("event collector exited");

    // ── 1. The engine ran ≥ 2 turns so the steered content had a
    // turn boundary at which to be drained.
    assert!(
        result.turns >= 2,
        "expected ≥2 turns so the queued message has a drain point; got turns={}",
        result.turns,
    );

    // ── 2. History stores the steered message as a RAW user message
    // (`MessageOrigin::QueuedSteering`), NOT a wrapped attachment. Mirrors TS:
    // the transcript shows the user's plain text; the model-facing framing is
    // applied only at prompt-build time (asserted in section 3).
    let steering_users: Vec<_> = result
        .final_messages
        .iter()
        .filter_map(|m| match m.as_ref() {
            coco_messages::Message::User(u)
                if u.origin == Some(coco_messages::MessageOrigin::QueuedSteering) =>
            {
                Some(u)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        steering_users.len(),
        1,
        "expected exactly one drained steering user message in history; found {}",
        steering_users.len()
    );
    let body = extract_all_text(&steering_users[0].message);
    assert!(
        body.contains(STEERING_MARKER),
        "stored steering message must carry the raw user text; body was: {body}"
    );
    assert!(
        !body.contains("<system-reminder>"),
        "stored steering message must be RAW — no <system-reminder> wrap in \
         history (the wrap is an API-serialization concern); body was: {body}"
    );
    assert!(
        !body.contains("The user sent a new message while you were working:"),
        "stored steering message must NOT carry the model-facing framing in \
         history; body was: {body}"
    );
    assert!(
        !result.final_messages.iter().any(|m| matches!(
            m.as_ref(),
            coco_messages::Message::Attachment(att) if att.kind == AttachmentKind::QueuedCommand
        )),
        "human steering must no longer land as a QueuedCommand attachment in history"
    );

    // ── 3. The next turn's API call saw the wrapped queued content in
    // its prompt — proving the wrapper is applied at prompt-build and the
    // content actually reached the model.
    let (prompt_count, second_prompt_text) = {
        let prompts = captured.lock().unwrap();
        let count = prompts.len();
        let text: String = prompts
            .get(1)
            .map(|p| p.iter().map(extract_all_text).collect())
            .unwrap_or_default();
        (count, text)
    };
    assert_eq!(
        prompt_count, 2,
        "mock should have observed both turns; got {prompt_count} calls"
    );
    assert!(
        second_prompt_text.contains(STEERING_MARKER),
        "second turn's prompt must contain the steered marker; \
         got prompt content: {second_prompt_text}"
    );
    assert!(
        second_prompt_text.contains("<system-reminder>"),
        "second turn's prompt must contain the <system-reminder> wrap \
         around the steered message; got prompt: {second_prompt_text}"
    );
    assert!(
        second_prompt_text.contains("The user sent a new message while you were working:"),
        "second turn's prompt must carry the model-facing steering framing \
         (applied at prompt-build, not stored); got prompt: {second_prompt_text}"
    );

    // ── 4. Lifecycle events fired exactly once each: one
    // CommandDequeued{id} per drained item plus one
    // QueueStateChanged{queued: 0} summary.
    let dequeue_events: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            CoreEvent::Protocol(ServerNotification::CommandDequeued { id }) => Some(id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        dequeue_events.len(),
        1,
        "expected one CommandDequeued event per drained item; got {}",
        dequeue_events.len()
    );
    let queue_state_events: Vec<i32> = events
        .iter()
        .filter_map(|e| match e {
            CoreEvent::Protocol(ServerNotification::QueueStateChanged { queued }) => Some(*queued),
            _ => None,
        })
        .collect();
    assert!(
        queue_state_events.contains(&0),
        "expected at least one QueueStateChanged{{queued: 0}} after drain; got events {queue_state_events:?}",
    );

    // ── 5. Final response references the marker, confirming the
    // model's reply was driven by the steered content.
    assert!(
        result.response_text.contains(STEERING_MARKER),
        "final response should reference the steered marker; got: {}",
        result.response_text
    );

    // ── 6. The shared queue is empty after drain — confirms
    // `remove_by_ids` actually clears entries (vs. duplicates leaking).
    assert!(
        queue.is_empty().await,
        "shared queue must be empty after end-of-turn drain"
    );
}

#[tokio::test]
async fn e2e_steering_origin_framing_per_kind() {
    // Direct unit-style coverage of the drain shape per origin tag,
    // without the cost of running an LLM turn. Confirms each
    // `QueueOrigin` variant gets its framing prose plus
    // the outer system-reminder wrap.
    use coco_messages::MessageHistory;

    let queue = CommandQueue::new();
    let mut history = MessageHistory::new();
    let (event_tx, _event_rx) = mpsc::channel::<CoreEvent>(16);
    let event_tx = Some(event_tx);

    queue
        .enqueue(
            QueuedCommand::new("hi human msg".into(), QueuePriority::Now)
                .with_origin(QueueOrigin::Human),
        )
        .await;
    queue
        .enqueue(
            QueuedCommand::new("hi coordinator msg".into(), QueuePriority::Next)
                .with_origin(QueueOrigin::Coordinator),
        )
        .await;
    queue
        .enqueue(
            QueuedCommand::new("hi task-notif msg".into(), QueuePriority::Later)
                .with_origin(QueueOrigin::TaskNotification),
        )
        .await;
    queue
        .enqueue(
            QueuedCommand::new("hi channel msg".into(), QueuePriority::Later).with_origin(
                QueueOrigin::Channel {
                    server: "test-mcp".into(),
                },
            ),
        )
        .await;

    coco_query::test_support::drain_into_history(
        &queue,
        &mut history,
        &event_tx,
        QueuePriority::Later,
        None,
    )
    .await;

    assert_eq!(
        history.len(),
        4,
        "all four queued items should drain into history"
    );
    let bodies: Vec<String> = history
        .iter()
        .map(|m| coco_messages::wrapping::extract_text_from_message(m))
        .collect();

    // Human steering (body[0]) is stored RAW — no framing, no wrap (it is a
    // plain user message; the wrapper is applied only at prompt-build).
    assert!(
        !bodies[0].contains("<system-reminder>"),
        "Human steering must be stored raw, not system-reminder wrapped; got: {}",
        bodies[0]
    );
    assert!(
        !bodies[0].contains("The user sent a new message while you were working:"),
        "Human steering must not carry the model-facing framing in history; got: {}",
        bodies[0]
    );

    // Non-human origins (coordinator / task-notification / channel) remain
    // model-only attachments, wrapped in <system-reminder> with their framing.
    for (idx, body) in bodies.iter().enumerate().skip(1) {
        assert!(
            body.contains("<system-reminder>"),
            "non-human body[{idx}] must be system-reminder wrapped; got: {body}"
        );
    }

    // Per-origin framing prose for the non-human items, in priority order.
    assert!(
        bodies[1].contains("The coordinator sent a message while you were working:"),
        "Coordinator origin prose missing from body[1]: {}",
        bodies[1]
    );
    assert!(
        bodies[2].contains("A background agent completed a task:"),
        "TaskNotification origin prose missing from body[2]: {}",
        bodies[2]
    );
    assert!(
        bodies[3].contains("A message arrived from test-mcp while you were working:"),
        "Channel origin prose missing from body[3]: {}",
        bodies[3]
    );

    // Sanity: the original user-typed payload survives into the wrap.
    assert!(bodies[0].contains("hi human msg"));
    assert!(bodies[1].contains("hi coordinator msg"));
    assert!(bodies[2].contains("hi task-notif msg"));
    assert!(bodies[3].contains("hi channel msg"));

    assert!(queue.is_empty().await);
}
