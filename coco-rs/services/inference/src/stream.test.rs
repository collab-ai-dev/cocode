use coco_llm_types::AssistantContentPart;
use coco_llm_types::FinishReason;
use coco_llm_types::ProviderMetadata;
use coco_llm_types::ReasoningPart;
use coco_llm_types::StopReason;
use coco_llm_types::TextPart;
use coco_llm_types::ToolCallPart;
use coco_llm_types::Usage;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc;
use vercel_ai_provider::AISdkError;
use vercel_ai_provider::LanguageModelV4StreamPart;
use vercel_ai_provider::LanguageModelV4ToolCall;
use vercel_ai_provider::StreamError;

use super::StreamEvent;
use super::default_process_stream_config;
use super::process_stream;
use super::process_stream_with_config;
use super::synthetic_stream_from_content;

/// Helper: build a one-key `ProviderMetadata` (e.g. `google.thoughtSignature`).
fn meta(provider: &str, key: &str, value: &str) -> ProviderMetadata {
    let mut outer: HashMap<String, serde_json::Value> = HashMap::new();
    outer.insert(provider.to_string(), serde_json::json!({ key: value }));
    ProviderMetadata::from_map(outer)
}

fn read_meta(pm: &ProviderMetadata, provider: &str, key: &str) -> Option<String> {
    pm.0.get(provider)?
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// End-to-end: feeding `synthetic_stream_from_content`'s output through
/// `process_stream` must yield `StreamEvent`s in the order reasoning → text →
/// tool input (start/delta/end) → Finish. This is the contract the agent
/// loop's per-turn consumer relies on.
#[tokio::test]
async fn synthetic_stream_emits_events_in_content_order() {
    let content = vec![
        AssistantContentPart::Reasoning(ReasoningPart {
            text: "thinking about it".into(),
            provider_metadata: None,
        }),
        AssistantContentPart::Text(TextPart {
            text: "Hello, world.".into(),
            provider_metadata: None,
        }),
        AssistantContentPart::ToolCall(ToolCallPart {
            tool_call_id: "call_42".into(),
            tool_name: "Bash".into(),
            input: serde_json::json!({"command": "echo hi"}),
            provider_executed: None,
            provider_metadata: None,
            invalid: false,
            invalid_reason: None,
        }),
    ];

    let stream_result = synthetic_stream_from_content(
        content,
        Usage::new(11, 7),
        FinishReason::new(StopReason::ToolUse),
    );

    let (tx, mut rx) = mpsc::channel::<StreamEvent>(32);
    tokio::spawn(process_stream(stream_result.stream, tx));

    let mut events = Vec::new();
    while let Some(ev) = rx.recv().await {
        events.push(ev);
    }

    // Expected: ReasoningDelta, ReasoningEnd, TextDelta, ToolCallStart,
    // ToolCallDelta, ToolCallEnd, Finish.
    //
    // The synthetic stream also emits `Part::ToolCall(tc)` as the
    // canonical close after `ToolInputEnd` (matching real Anthropic /
    // OpenAI Responses / Google streams), but `stream_event_from_part`
    // doesn't surface a corresponding `StreamEvent` for that variant
    // — the accumulator updates `snapshot.tool_calls` via the
    // `LanguageModelV4StreamPart::ToolCall` path instead. So the
    // observable event count from `process_stream` remains 7.
    //
    // `ReasoningEnd` is emitted by `process_stream` as a distinct
    // `StreamEvent::ReasoningEnd`; only `TextStart`/`TextEnd` and
    // `ReasoningStart` brackets are filtered out.
    assert_eq!(events.len(), 7);
    assert!(
        matches!(&events[0], StreamEvent::ReasoningDelta { text } if text == "thinking about it"),
        "first event should be ReasoningDelta, got {:?}",
        &events[0]
    );
    assert!(
        matches!(&events[1], StreamEvent::ReasoningEnd { .. }),
        "second event should be ReasoningEnd, got {:?}",
        &events[1]
    );
    assert!(
        matches!(&events[2], StreamEvent::TextDelta { text } if text == "Hello, world."),
        "third event should be TextDelta, got {:?}",
        &events[2]
    );
    assert!(
        matches!(
            &events[3],
            StreamEvent::ToolCallStart { id, tool_name }
                if id == "call_42" && tool_name == "Bash"
        ),
        "fourth event should be ToolCallStart, got {:?}",
        &events[3]
    );
    assert!(
        matches!(&events[4], StreamEvent::ToolCallDelta { id, delta }
            if id == "call_42" && delta.contains("echo hi")),
        "fifth event should be ToolCallDelta with serialized input, got {:?}",
        &events[4]
    );
    assert!(
        matches!(&events[5], StreamEvent::ToolCallEnd { id } if id == "call_42"),
        "sixth event should be ToolCallEnd, got {:?}",
        &events[5]
    );
    match &events[6] {
        StreamEvent::Finish {
            metrics, snapshot, ..
        } => {
            assert!(metrics.ttft_ms.is_some());
            assert_eq!(metrics.stall_count, 0);
            assert_eq!(metrics.total_stall_ms, 0);
            // Snapshot must mirror the input content: 3 parts in
            // emission order (Reasoning, Text, ToolCall) with the
            // tool call marked complete (close event was sent).
            assert_eq!(snapshot.parts.len(), 3);
            match &snapshot.parts[2] {
                super::TurnPart::ToolCall(tc) => {
                    assert!(
                        tc.is_complete,
                        "tool call should be complete after ToolCall close"
                    );
                    assert!(tc.is_input_complete);
                    assert_eq!(tc.tool_name, "Bash");
                }
                other => panic!("third part should be ToolCall, got {other:?}"),
            }
        }
        other => panic!("last event should be Finish, got {other:?}"),
    }
}

#[tokio::test]
async fn process_stream_reports_provider_stream_errors_with_metrics() {
    let parts: Vec<Result<LanguageModelV4StreamPart, AISdkError>> = vec![
        Ok(LanguageModelV4StreamPart::TextDelta {
            id: "t1".into(),
            delta: "partial".into(),
            provider_metadata: None,
        }),
        Err(AISdkError::new("connection reset")),
    ];

    let (tx, mut rx) = mpsc::channel::<StreamEvent>(16);
    tokio::spawn(process_stream(Box::pin(futures::stream::iter(parts)), tx));

    assert!(matches!(
        rx.recv().await,
        Some(StreamEvent::TextDelta { text }) if text == "partial"
    ));
    match rx.recv().await {
        Some(StreamEvent::Error { message, metrics }) => {
            assert!(message.contains("connection reset"));
            assert!(metrics.ttft_ms.is_some());
        }
        other => panic!("expected stream error, got {other:?}"),
    }
}

#[tokio::test]
async fn process_stream_reports_error_parts_with_metrics() {
    let parts: Vec<Result<LanguageModelV4StreamPart, AISdkError>> = vec![
        Ok(LanguageModelV4StreamPart::ToolInputStart {
            id: "call_1".into(),
            tool_name: "Read".into(),
            provider_executed: None,
            dynamic: None,
            title: None,
            provider_metadata: None,
        }),
        Ok(LanguageModelV4StreamPart::Error {
            error: StreamError::new("provider overloaded"),
        }),
    ];

    let (tx, mut rx) = mpsc::channel::<StreamEvent>(16);
    tokio::spawn(process_stream(Box::pin(futures::stream::iter(parts)), tx));

    assert!(matches!(
        rx.recv().await,
        Some(StreamEvent::ToolCallStart { id, tool_name })
            if id == "call_1" && tool_name == "Read"
    ));
    match rx.recv().await {
        Some(StreamEvent::Error { message, metrics }) => {
            assert_eq!(message, "provider overloaded");
            assert!(metrics.ttft_ms.is_some());
        }
        other => panic!("expected error part, got {other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn process_stream_with_config_uses_custom_stall_threshold() {
    let stream = futures::stream::unfold(0usize, |idx| async move {
        if idx == 1 {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        let part = match idx {
            0 => LanguageModelV4StreamPart::TextDelta {
                id: "t1".into(),
                delta: "a".into(),
                provider_metadata: None,
            },
            1 => LanguageModelV4StreamPart::TextDelta {
                id: "t1".into(),
                delta: "b".into(),
                provider_metadata: None,
            },
            2 => LanguageModelV4StreamPart::Finish {
                usage: Usage::new(1, 2),
                finish_reason: FinishReason::new(StopReason::EndTurn),
                provider_metadata: None,
            },
            _ => return None,
        };
        Some((Ok(part), idx + 1))
    });
    let config = default_process_stream_config().with_stall_threshold(Duration::from_secs(1));

    let (tx, mut rx) = mpsc::channel::<StreamEvent>(16);
    tokio::spawn(process_stream_with_config(Box::pin(stream), tx, config));

    let mut finish_metrics = None;
    while let Some(event) = rx.recv().await {
        if let StreamEvent::Finish { metrics, .. } = event {
            finish_metrics = Some(metrics);
            break;
        }
    }

    let metrics = finish_metrics.expect("finish event should include metrics");
    assert_eq!(metrics.stall_count, 1);
    assert_eq!(metrics.total_stall_ms, 2_000);
}

// ─── AssistantTurnSnapshot accumulator tests ─────────────────────────
//
// The accumulator is private; we exercise it through the public
// `process_stream` path by inspecting `StreamEvent::Finish.snapshot`.

/// Provider metadata on every emitted variant survives into the
/// per-segment snapshot. This is the load-bearing invariant for
/// Gemini `thoughtSignature` / Anthropic `signature` / OpenAI
/// `encrypted_content` round-trip.
#[tokio::test]
async fn snapshot_preserves_provider_metadata_on_every_variant() {
    let reasoning_meta = meta("anthropic", "signature", "S1");
    let tool_meta = meta("google", "thoughtSignature", "T1");
    let text_meta = meta("google", "thoughtSignature", "Tx");

    let content = vec![
        AssistantContentPart::Reasoning(ReasoningPart {
            text: "thinking...".into(),
            provider_metadata: Some(reasoning_meta.clone()),
        }),
        AssistantContentPart::Text(TextPart {
            text: "answer prefix".into(),
            provider_metadata: Some(text_meta.clone()),
        }),
        AssistantContentPart::ToolCall(ToolCallPart {
            tool_call_id: "call_42".into(),
            tool_name: "Bash".into(),
            input: serde_json::json!({"command": "ls"}),
            provider_executed: None,
            provider_metadata: Some(tool_meta.clone()),
            invalid: false,
            invalid_reason: None,
        }),
    ];

    let stream_result = synthetic_stream_from_content(
        content,
        Usage::new(10, 5),
        FinishReason::new(StopReason::ToolUse),
    );
    let (tx, mut rx) = mpsc::channel::<StreamEvent>(64);
    tokio::spawn(process_stream(stream_result.stream, tx));

    let mut snapshot = None;
    while let Some(ev) = rx.recv().await {
        if let StreamEvent::Finish { snapshot: snap, .. } = ev {
            snapshot = Some(snap);
        }
    }
    let snap = snapshot.expect("Finish event should carry a snapshot");

    assert_eq!(snap.parts.len(), 3, "3 parts in emission order");

    match &snap.parts[0] {
        super::TurnPart::Reasoning(r) => {
            assert_eq!(r.text, "thinking...");
            let pm = r.provider_metadata.as_ref().expect("reasoning metadata");
            assert_eq!(read_meta(pm, "anthropic", "signature"), Some("S1".into()));
        }
        other => panic!("expected Reasoning, got {other:?}"),
    }
    match &snap.parts[1] {
        super::TurnPart::Text(t) => {
            assert_eq!(t.text, "answer prefix");
            let pm = t.provider_metadata.as_ref().expect("text metadata");
            assert_eq!(
                read_meta(pm, "google", "thoughtSignature"),
                Some("Tx".into())
            );
        }
        other => panic!("expected Text, got {other:?}"),
    }
    match &snap.parts[2] {
        super::TurnPart::ToolCall(tc) => {
            assert!(tc.is_complete);
            let pm = tc.provider_metadata.as_ref().expect("tool call metadata");
            assert_eq!(
                read_meta(pm, "google", "thoughtSignature"),
                Some("T1".into())
            );
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }
}

/// Multiple reasoning segments per turn preserve their own metadata.
/// Anthropic interleaved thinking and OpenAI Responses multi-item
/// reasoning both produce this shape on the wire.
#[tokio::test]
async fn snapshot_preserves_multiple_reasoning_segments() {
    let parts: Vec<Result<LanguageModelV4StreamPart, AISdkError>> = vec![
        Ok(LanguageModelV4StreamPart::ReasoningStart {
            id: "r1".into(),
            provider_metadata: None,
        }),
        Ok(LanguageModelV4StreamPart::ReasoningDelta {
            id: "r1".into(),
            delta: "first thought".into(),
            provider_metadata: None,
        }),
        Ok(LanguageModelV4StreamPart::ReasoningEnd {
            id: "r1".into(),
            provider_metadata: Some(meta("anthropic", "signature", "S1")),
        }),
        Ok(LanguageModelV4StreamPart::ReasoningStart {
            id: "r2".into(),
            provider_metadata: None,
        }),
        Ok(LanguageModelV4StreamPart::ReasoningDelta {
            id: "r2".into(),
            delta: "second thought".into(),
            provider_metadata: None,
        }),
        Ok(LanguageModelV4StreamPart::ReasoningEnd {
            id: "r2".into(),
            provider_metadata: Some(meta("anthropic", "signature", "S2")),
        }),
        Ok(LanguageModelV4StreamPart::Finish {
            usage: Usage::new(1, 1),
            finish_reason: FinishReason::new(StopReason::EndTurn),
            provider_metadata: None,
        }),
    ];

    let (tx, mut rx) = mpsc::channel::<StreamEvent>(32);
    tokio::spawn(process_stream(Box::pin(futures::stream::iter(parts)), tx));

    let mut snapshot = None;
    while let Some(ev) = rx.recv().await {
        if let StreamEvent::Finish { snapshot: snap, .. } = ev {
            snapshot = Some(snap);
        }
    }
    let snap = snapshot.expect("Finish snapshot");
    assert_eq!(snap.parts.len(), 2, "two reasoning segments");
    let signatures: Vec<String> = snap
        .parts
        .iter()
        .filter_map(|p| match p {
            super::TurnPart::Reasoning(r) => r
                .provider_metadata
                .as_ref()
                .and_then(|pm| read_meta(pm, "anthropic", "signature")),
            _ => None,
        })
        .collect();
    assert_eq!(signatures, vec!["S1", "S2"]);
}

/// Interleaved emission order is preserved: `[Text(A), ToolCall, Text(B)]`
/// must stay in that order — not collapsed or canonicalized.
#[tokio::test]
async fn snapshot_preserves_text_tool_text_interleaving() {
    let parts: Vec<Result<LanguageModelV4StreamPart, AISdkError>> = vec![
        Ok(LanguageModelV4StreamPart::TextStart {
            id: "t1".into(),
            provider_metadata: None,
        }),
        Ok(LanguageModelV4StreamPart::TextDelta {
            id: "t1".into(),
            delta: "before".into(),
            provider_metadata: None,
        }),
        Ok(LanguageModelV4StreamPart::TextEnd {
            id: "t1".into(),
            provider_metadata: None,
        }),
        Ok(LanguageModelV4StreamPart::ToolInputStart {
            id: "call_1".into(),
            tool_name: "Bash".into(),
            provider_executed: None,
            dynamic: None,
            title: None,
            provider_metadata: None,
        }),
        Ok(LanguageModelV4StreamPart::ToolInputDelta {
            id: "call_1".into(),
            delta: r#"{"command":"ls"}"#.into(),
            provider_metadata: None,
        }),
        Ok(LanguageModelV4StreamPart::ToolInputEnd {
            id: "call_1".into(),
            provider_metadata: None,
        }),
        Ok(LanguageModelV4StreamPart::TextStart {
            id: "t2".into(),
            provider_metadata: None,
        }),
        Ok(LanguageModelV4StreamPart::TextDelta {
            id: "t2".into(),
            delta: "after".into(),
            provider_metadata: None,
        }),
        Ok(LanguageModelV4StreamPart::TextEnd {
            id: "t2".into(),
            provider_metadata: None,
        }),
        Ok(LanguageModelV4StreamPart::Finish {
            usage: Usage::new(1, 1),
            finish_reason: FinishReason::new(StopReason::ToolUse),
            provider_metadata: None,
        }),
    ];

    let (tx, mut rx) = mpsc::channel::<StreamEvent>(32);
    tokio::spawn(process_stream(Box::pin(futures::stream::iter(parts)), tx));

    let mut snapshot = None;
    while let Some(ev) = rx.recv().await {
        if let StreamEvent::Finish { snapshot: snap, .. } = ev {
            snapshot = Some(snap);
        }
    }
    let snap = snapshot.expect("Finish snapshot");
    assert_eq!(snap.parts.len(), 3);
    assert!(matches!(&snap.parts[0], super::TurnPart::Text(t) if t.text == "before"));
    assert!(matches!(&snap.parts[1], super::TurnPart::ToolCall(_)));
    assert!(matches!(&snap.parts[2], super::TurnPart::Text(t) if t.text == "after"));
}

// ───────────────────────────────────────────────────────────────────────────
// Source-contract pin (tui-v2 §8 / §10.1 Stage 0).
//
// The TUI v2 anchored finalize (`docs/coco-rs/ui/tui-v2-design.md` §6.2) drops
// the rasterized row-fingerprint re-verification and instead anchors the
// streamed scrollback rows against the canonical message at the SOURCE level
// (`AssistantText.text.starts_with(emitted_source_prefix)`). That is sound only
// if the canonical text the TUI anchors against is byte-identical to the source
// the stream accumulated. These tests pin that contract at the layer where it
// holds: each `TurnPart::Text` segment's `.text` is exactly the ordered byte
// concatenation of that part's own `TextDelta` run
// (`AssistantTurnSnapshotState::update`, accumulate-per-id by stream id).
//
// Part identity is erased one layer up — `stream_event_from_part` flattens
// every `TextDelta` into a bare `StreamEvent::TextDelta { text }` with no part
// id, so `app/query` cannot observe per-part boundaries and the pin cannot live
// there. Known asymmetry (documented, not papered over): `emit_stream`
// (`app/query/.../engine_stream_consume.rs`) discards the `#[must_use]` send
// result. The underlying `mpsc::Sender::send(..).await` blocks on backpressure
// and fails only once the TUI receiver is dropped (shutdown) — it cannot lose
// a delta transiently while the TUI lives. Scope of the downstream guard,
// stated precisely: canonical-vs-streamed divergence WITHIN the committed
// prefix is caught by the finalize source-anchor (`starts_with`) → replay;
// divergence past the committed prefix is unguarded by construction (tui-v2
// §6.2 residual class — it would need a mid-turn delta loss this channel
// cannot produce).

/// Drive `parts` through `process_stream` and return the `Finish` snapshot.
async fn finish_snapshot(
    parts: Vec<Result<LanguageModelV4StreamPart, AISdkError>>,
) -> std::sync::Arc<super::AssistantTurnSnapshot> {
    let (tx, mut rx) = mpsc::channel::<StreamEvent>(64);
    tokio::spawn(process_stream(Box::pin(futures::stream::iter(parts)), tx));
    let mut snapshot = None;
    while let Some(ev) = rx.recv().await {
        if let StreamEvent::Finish { snapshot: snap, .. } = ev {
            snapshot = Some(snap);
        }
    }
    snapshot.expect("Finish snapshot")
}

fn text_part(part: &super::TurnPart) -> &str {
    match part {
        super::TurnPart::Text(t) => &t.text,
        other => panic!("expected Text part, got {other:?}"),
    }
}

fn text_start(id: &str) -> Result<LanguageModelV4StreamPart, AISdkError> {
    Ok(LanguageModelV4StreamPart::TextStart {
        id: id.into(),
        provider_metadata: None,
    })
}

fn text_delta(id: &str, delta: &str) -> Result<LanguageModelV4StreamPart, AISdkError> {
    Ok(LanguageModelV4StreamPart::TextDelta {
        id: id.into(),
        delta: delta.into(),
        provider_metadata: None,
    })
}

fn text_end(id: &str) -> Result<LanguageModelV4StreamPart, AISdkError> {
    Ok(LanguageModelV4StreamPart::TextEnd {
        id: id.into(),
        provider_metadata: None,
    })
}

fn finish(stop: StopReason) -> Result<LanguageModelV4StreamPart, AISdkError> {
    Ok(LanguageModelV4StreamPart::Finish {
        usage: Usage::new(1, 1),
        finish_reason: FinishReason::new(stop),
        provider_metadata: None,
    })
}

/// Single run: one `Text` part whose source is split across several
/// `TextDelta` chunks accumulates to their exact in-order concatenation.
#[tokio::test]
async fn source_contract_text_part_equals_concat_of_single_delta_run() {
    let parts = vec![
        text_start("t1"),
        text_delta("t1", "The "),
        text_delta("t1", "quick "),
        text_delta("t1", "brown "),
        text_delta("t1", "fox"),
        text_end("t1"),
        finish(StopReason::EndTurn),
    ];

    let snap = finish_snapshot(parts).await;
    assert_eq!(snap.parts.len(), 1);
    assert_eq!(text_part(&snap.parts[0]), "The quick brown fox");
}

/// `text → tool → text` in ONE turn: each `Text` part stays isolated to its own
/// `TextDelta` run — the tool boundary never bleeds `t1`'s deltas into `t2` or
/// vice versa. This is the per-segment equality the TUI anchor relies on for
/// the §6.2.1 within-message shape, where the streamed prefix of one segment
/// must never anchor against a different segment's canonical text.
#[tokio::test]
async fn source_contract_text_tool_text_each_part_isolated() {
    let parts = vec![
        text_start("t1"),
        text_delta("t1", "be"),
        text_delta("t1", "fore"),
        text_end("t1"),
        Ok(LanguageModelV4StreamPart::ToolInputStart {
            id: "call_1".into(),
            tool_name: "Bash".into(),
            provider_executed: None,
            dynamic: None,
            title: None,
            provider_metadata: None,
        }),
        Ok(LanguageModelV4StreamPart::ToolInputDelta {
            id: "call_1".into(),
            delta: r#"{"command":"ls"}"#.into(),
            provider_metadata: None,
        }),
        Ok(LanguageModelV4StreamPart::ToolInputEnd {
            id: "call_1".into(),
            provider_metadata: None,
        }),
        text_start("t2"),
        text_delta("t2", "af"),
        text_delta("t2", "ter"),
        text_end("t2"),
        finish(StopReason::ToolUse),
    ];

    let snap = finish_snapshot(parts).await;
    assert_eq!(snap.parts.len(), 3);
    assert_eq!(text_part(&snap.parts[0]), "before");
    assert!(matches!(&snap.parts[1], super::TurnPart::ToolCall(_)));
    assert_eq!(text_part(&snap.parts[2]), "after");
}

/// Interrupted turn: a `Text` run that never receives its `TextEnd` before the
/// turn finishes (the cancel shape — the engine later rebuilds a single Text
/// part from the accumulated `response_text`) still accumulates its partial
/// deltas exactly. The snapshot keeps the in-flight segment with its
/// byte-accurate prefix, which is what the finalize anchor compares against.
#[tokio::test]
async fn source_contract_interrupted_run_keeps_byte_accurate_partial() {
    let parts = vec![
        text_start("t1"),
        text_delta("t1", "half "),
        text_delta("t1", "a thought"),
        // No TextEnd — the turn is cut short before the run closes.
        finish(StopReason::Other),
    ];

    let snap = finish_snapshot(parts).await;
    assert_eq!(snap.parts.len(), 1);
    assert_eq!(text_part(&snap.parts[0]), "half a thought");
}

/// Empty `Text` part (`TextStart`+`TextEnd`, no delta) is kept at the inference
/// snapshot as an empty-text segment; the symmetric drop of empty parts happens
/// downstream in reconstruction (`app/query` engine) and TUI derive, NOT here.
/// Pinned so a future "skip empty" optimization at this layer is a deliberate,
/// visible change rather than a silent divergence.
#[tokio::test]
async fn source_contract_empty_text_part_kept_at_inference_layer() {
    let parts = vec![
        text_start("t1"),
        text_end("t1"),
        finish(StopReason::EndTurn),
    ];

    let snap = finish_snapshot(parts).await;
    assert_eq!(snap.parts.len(), 1);
    assert_eq!(text_part(&snap.parts[0]), "");
}

/// `ToolInputStart` + `ToolInputDelta` + `ToolInputEnd` WITHOUT a
/// terminal `ToolCall(tc)` close — `is_input_complete=true` but
/// `is_complete=false`. Some providers / mocks produce this shape;
/// the snapshot must still surface the tool call.
#[tokio::test]
async fn snapshot_handles_tool_call_without_close_event() {
    let parts: Vec<Result<LanguageModelV4StreamPart, AISdkError>> = vec![
        Ok(LanguageModelV4StreamPart::ToolInputStart {
            id: "call_1".into(),
            tool_name: "Bash".into(),
            provider_executed: None,
            dynamic: None,
            title: None,
            provider_metadata: None,
        }),
        Ok(LanguageModelV4StreamPart::ToolInputDelta {
            id: "call_1".into(),
            delta: r#"{"command":"echo"}"#.into(),
            provider_metadata: None,
        }),
        Ok(LanguageModelV4StreamPart::ToolInputEnd {
            id: "call_1".into(),
            provider_metadata: None,
        }),
        // No ToolCall(tc) close.
        Ok(LanguageModelV4StreamPart::Finish {
            usage: Usage::new(1, 1),
            finish_reason: FinishReason::new(StopReason::ToolUse),
            provider_metadata: None,
        }),
    ];

    let (tx, mut rx) = mpsc::channel::<StreamEvent>(16);
    tokio::spawn(process_stream(Box::pin(futures::stream::iter(parts)), tx));

    let mut snapshot = None;
    while let Some(ev) = rx.recv().await {
        if let StreamEvent::Finish { snapshot: snap, .. } = ev {
            snapshot = Some(snap);
        }
    }
    let snap = snapshot.expect("Finish snapshot");
    assert_eq!(snap.parts.len(), 1);
    match &snap.parts[0] {
        super::TurnPart::ToolCall(tc) => {
            assert!(tc.is_input_complete);
            assert!(!tc.is_complete);
            assert_eq!(tc.input_json, r#"{"command":"echo"}"#);
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }
}

/// `ToolCall(tc)` close with `provider_metadata = None` MUST NOT
/// overwrite earlier `ToolInputStart`'s `Some(...)`. Without this
/// rule, Gemini's signature on ToolInputStart would be lost any time
/// the close event omitted metadata (rare in practice but a real bug).
#[tokio::test]
async fn snapshot_none_close_does_not_overwrite_some_start() {
    let some_meta = meta("google", "thoughtSignature", "T1");
    let parts: Vec<Result<LanguageModelV4StreamPart, AISdkError>> = vec![
        Ok(LanguageModelV4StreamPart::ToolInputStart {
            id: "call_1".into(),
            tool_name: "Bash".into(),
            provider_executed: None,
            dynamic: None,
            title: None,
            provider_metadata: Some(some_meta.clone()),
        }),
        Ok(LanguageModelV4StreamPart::ToolInputDelta {
            id: "call_1".into(),
            delta: r#"{"command":"ls"}"#.into(),
            provider_metadata: None,
        }),
        Ok(LanguageModelV4StreamPart::ToolInputEnd {
            id: "call_1".into(),
            provider_metadata: None,
        }),
        Ok(LanguageModelV4StreamPart::ToolCall(
            LanguageModelV4ToolCall {
                tool_call_id: "call_1".into(),
                tool_name: "Bash".into(),
                input: r#"{"command":"ls"}"#.into(),
                provider_executed: None,
                dynamic: None,
                provider_metadata: None, // ← critical: None close
                invalid: false,
                invalid_reason: None,
            },
        )),
        Ok(LanguageModelV4StreamPart::Finish {
            usage: Usage::new(1, 1),
            finish_reason: FinishReason::new(StopReason::ToolUse),
            provider_metadata: None,
        }),
    ];

    let (tx, mut rx) = mpsc::channel::<StreamEvent>(16);
    tokio::spawn(process_stream(Box::pin(futures::stream::iter(parts)), tx));

    let mut snapshot = None;
    while let Some(ev) = rx.recv().await {
        if let StreamEvent::Finish { snapshot: snap, .. } = ev {
            snapshot = Some(snap);
        }
    }
    let snap = snapshot.expect("Finish snapshot");
    match &snap.parts[0] {
        super::TurnPart::ToolCall(tc) => {
            assert!(tc.is_complete, "close arrived");
            let pm = tc
                .provider_metadata
                .as_ref()
                .expect("Some(start metadata) must survive None close");
            assert_eq!(
                read_meta(pm, "google", "thoughtSignature"),
                Some("T1".into())
            );
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }
}

/// Duplicate `*Start` events (provider bug) — accumulator must
/// remain idempotent: ignore the second start and keep the first
/// segment. Defensive; real providers don't emit this.
#[tokio::test]
async fn snapshot_idempotent_on_duplicate_text_start() {
    let parts: Vec<Result<LanguageModelV4StreamPart, AISdkError>> = vec![
        Ok(LanguageModelV4StreamPart::TextStart {
            id: "t1".into(),
            provider_metadata: None,
        }),
        Ok(LanguageModelV4StreamPart::TextStart {
            id: "t1".into(),
            provider_metadata: None,
        }),
        Ok(LanguageModelV4StreamPart::TextDelta {
            id: "t1".into(),
            delta: "hello".into(),
            provider_metadata: None,
        }),
        Ok(LanguageModelV4StreamPart::TextEnd {
            id: "t1".into(),
            provider_metadata: None,
        }),
        Ok(LanguageModelV4StreamPart::Finish {
            usage: Usage::new(1, 1),
            finish_reason: FinishReason::new(StopReason::EndTurn),
            provider_metadata: None,
        }),
    ];

    let (tx, mut rx) = mpsc::channel::<StreamEvent>(16);
    tokio::spawn(process_stream(Box::pin(futures::stream::iter(parts)), tx));

    let mut snapshot = None;
    while let Some(ev) = rx.recv().await {
        if let StreamEvent::Finish { snapshot: snap, .. } = ev {
            snapshot = Some(snap);
        }
    }
    let snap = snapshot.expect("Finish snapshot");
    assert_eq!(snap.parts.len(), 1, "duplicate Start should not add a part");
}

/// Tool input must round-trip through the synthetic stream: delta JSON parsed
/// back into a `Value` equivalent to the original input. Guards against
/// regressions in the delta-chunking strategy (currently single-shot but the
/// contract allows multi-chunk).
#[tokio::test]
async fn tool_input_round_trips_through_synthetic_stream() {
    let original_input = serde_json::json!({
        "command": "ls -la",
        "cwd": "/tmp",
        "timeout_ms": 5000
    });

    let content = vec![AssistantContentPart::ToolCall(ToolCallPart {
        tool_call_id: "rt".into(),
        tool_name: "Bash".into(),
        input: original_input.clone(),
        provider_executed: None,
        invalid: false,
        invalid_reason: None,
        provider_metadata: None,
    })];

    let stream_result = synthetic_stream_from_content(
        content,
        Usage::new(5, 3),
        FinishReason::new(StopReason::ToolUse),
    );

    // Collect all deltas for the tool call.
    let mut json_accumulator = String::new();
    let mut stream = stream_result.stream;
    while let Some(part) = stream.next().await {
        if let Ok(vercel_ai_provider::LanguageModelV4StreamPart::ToolInputDelta { delta, .. }) =
            part
        {
            json_accumulator.push_str(&delta);
        }
    }

    let round_tripped: serde_json::Value =
        serde_json::from_str(&json_accumulator).expect("accumulated JSON should parse");
    assert_eq!(round_tripped, original_input);
}
