//! Streaming query support — processes LanguageModelV4StreamPart into StreamEvent.
//!
//! End-of-turn assistant reconstruction reads `StreamEvent::Finish.snapshot`,
//! an `Arc<AssistantTurnSnapshot>` that captures per-part `provider_metadata`
//! (Gemini `thoughtSignature`, Anthropic `signature`, OpenAI `encrypted_content`)
//! verbatim. See `docs/coco-rs/streaming-metadata-roundtrip-plan.md`.

use coco_llm_types::AssistantContentPart;
use coco_llm_types::FinishReason;
use coco_llm_types::ProviderMetadata;
use coco_llm_types::Usage;
use coco_types::TokenUsage;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::debug;
use tracing::trace;
use tracing::warn;
use vercel_ai::StreamProcessor;
use vercel_ai_provider::AISdkError;
use vercel_ai_provider::LanguageModelV4FileData;
use vercel_ai_provider::LanguageModelV4StreamPart;
use vercel_ai_provider::LanguageModelV4StreamResult;

pub use vercel_ai::StreamMetrics;
pub use vercel_ai::StreamProcessorConfig;

const DEFAULT_PROCESS_STREAM_STALL_THRESHOLD: Duration = Duration::from_secs(30);

// ─── AssistantTurnSnapshot — per-turn full-fidelity assistant content ────
//
// Owned by coco-inference. Accumulated inside `process_stream_with_config`
// as parts arrive, then shipped as `Arc<AssistantTurnSnapshot>` on
// `StreamEvent::Finish.snapshot`. The agent loop in `coco-query::engine`
// walks `parts` in order to rebuild `Vec<AssistantContentPart>` with every
// part's `provider_metadata` preserved verbatim.
//
// Accumulation lives here, not in vercel-ai/ai, because it embeds policy
// (which parts matter, which metadata to preserve). vercel-ai/ai's job
// ends at `Stream<LanguageModelV4StreamPart>` + idle-timeout + metrics —
// content accumulation is consumer policy, not vendor protocol.

/// One emitted segment in an assistant turn, paired with the
/// provider metadata that was attached to it on the wire.
#[derive(Debug, Clone)]
pub struct TextSegment {
    pub id: String,
    pub text: String,
    pub provider_metadata: Option<ProviderMetadata>,
}

#[derive(Debug, Clone)]
pub struct ReasoningSegment {
    pub id: String,
    pub text: String,
    pub provider_metadata: Option<ProviderMetadata>,
}

#[derive(Debug, Clone)]
pub struct ToolCallSegment {
    pub id: String,
    pub tool_name: String,
    /// Stringified JSON accumulated from `ToolInputDelta` events.
    /// When `ToolCall(tc)` close arrives, this is overwritten with
    /// `tc.input` (canonical close).
    pub input_json: String,
    pub provider_executed: Option<bool>,
    pub dynamic: Option<bool>,
    /// `true` once `ToolInputEnd` has arrived.
    pub is_input_complete: bool,
    /// `true` once `ToolCall(tc)` close has arrived. Some providers
    /// (and the synthetic stream helper) omit this — reconstruction
    /// filters on `is_input_complete || is_complete`.
    pub is_complete: bool,
    pub provider_metadata: Option<ProviderMetadata>,
    /// Coco-rs extension carried through from
    /// [`vercel_ai_provider::LanguageModelV4ToolCall.invalid`]: the
    /// adapter detected an unrecoverable wire-parsing parse failure on
    /// the wire. Engine reconstruction copies this onto the rebuilt
    /// [`coco_llm_types::ToolCallPart`] so the agent loop can pick
    /// the right `<tool_use_error>` wrap prefix without going through
    /// schema validation first.
    pub invalid: bool,
    /// Structured reason accompanying [`Self::invalid`]; set by the
    /// provider adapter at the wire boundary.
    pub invalid_reason: Option<vercel_ai_provider::ToolInputInvalidReason>,
}

#[derive(Debug, Clone)]
pub struct FileSegment {
    pub id: String,
    /// Always wrapped as `Data { data }` since `LanguageModelV4StreamPart::File`
    /// carries `data: String` without a URL/base64 discriminator.
    pub data: LanguageModelV4FileData,
    pub media_type: String,
    pub provider_metadata: Option<ProviderMetadata>,
}

#[derive(Debug, Clone)]
pub struct ReasoningFileSegment {
    pub id: String,
    pub data: LanguageModelV4FileData,
    pub media_type: String,
    pub provider_metadata: Option<ProviderMetadata>,
}

#[derive(Debug, Clone)]
pub struct SourceSegment {
    pub id: String,
    pub url: Option<String>,
    pub title: Option<String>,
    pub source_type: String,
    pub provider_metadata: Option<ProviderMetadata>,
}

#[derive(Debug, Clone)]
pub struct CustomSegment {
    pub id: String,
    pub data: serde_json::Value,
    pub provider_metadata: Option<ProviderMetadata>,
}

#[derive(Debug, Clone)]
pub struct ToolApprovalRequestSegment {
    pub approval_id: String,
    pub tool_call_id: String,
    pub provider_metadata: Option<ProviderMetadata>,
}

/// One assistant-turn part, preserving emission order across kinds.
#[derive(Debug, Clone)]
pub enum TurnPart {
    Text(TextSegment),
    Reasoning(ReasoningSegment),
    ToolCall(ToolCallSegment),
    File(FileSegment),
    ReasoningFile(ReasoningFileSegment),
    Source(SourceSegment),
    Custom(CustomSegment),
    ToolApprovalRequest(ToolApprovalRequestSegment),
}

/// Full reconstruction of one assistant turn. Shipped on
/// `StreamEvent::Finish.snapshot` as `Arc<AssistantTurnSnapshot>`.
#[derive(Debug, Clone, Default)]
pub struct AssistantTurnSnapshot {
    pub parts: Vec<TurnPart>,
}

/// Internal accumulator. Mutated as `LanguageModelV4StreamPart` events
/// arrive; produces the final `AssistantTurnSnapshot` on `Finish`.
#[derive(Default)]
struct AssistantTurnSnapshotState {
    snapshot: AssistantTurnSnapshot,
    /// Index in `snapshot.parts` for the active text/reasoning/tool segment
    /// keyed by stream id. `ToolApprovalRequest` etc. don't need active
    /// tracking — they're pushed atomically.
    active_text: HashMap<String, usize>,
    active_reasoning: HashMap<String, usize>,
    active_tool: HashMap<String, usize>,
}

impl AssistantTurnSnapshotState {
    fn new() -> Self {
        Self::default()
    }

    /// Merge `incoming` into `existing` using first-wins semantics
    /// (Gemini repeats identical blob on every delta; A4 from review).
    fn merge_metadata_first_wins(
        existing: &mut Option<ProviderMetadata>,
        incoming: Option<&ProviderMetadata>,
    ) {
        if existing.is_none()
            && let Some(m) = incoming
        {
            *existing = Some(m.clone());
        }
    }

    /// Pattern-match on every relevant `LanguageModelV4StreamPart` variant
    /// and update the snapshot. Pure function over state + part — no I/O.
    fn update(&mut self, part: &LanguageModelV4StreamPart) {
        match part {
            // ─── Text ────────────────────────────────────────────────
            LanguageModelV4StreamPart::TextStart {
                id,
                provider_metadata,
            } => {
                // Idempotency: duplicate *Start is a provider bug; skip.
                if self.active_text.contains_key(id) {
                    trace!(id = %id, "duplicate TextStart ignored");
                    return;
                }
                let idx = self.snapshot.parts.len();
                self.snapshot.parts.push(TurnPart::Text(TextSegment {
                    id: id.clone(),
                    text: String::new(),
                    provider_metadata: provider_metadata.clone(),
                }));
                self.active_text.insert(id.clone(), idx);
            }
            LanguageModelV4StreamPart::TextDelta {
                id,
                delta,
                provider_metadata,
            } => {
                if let Some(&idx) = self.active_text.get(id)
                    && let Some(TurnPart::Text(seg)) = self.snapshot.parts.get_mut(idx)
                {
                    seg.text.push_str(delta);
                    Self::merge_metadata_first_wins(
                        &mut seg.provider_metadata,
                        provider_metadata.as_ref(),
                    );
                }
            }
            LanguageModelV4StreamPart::TextEnd {
                id,
                provider_metadata,
            } => {
                if let Some(idx) = self.active_text.remove(id)
                    && let Some(TurnPart::Text(seg)) = self.snapshot.parts.get_mut(idx)
                {
                    Self::merge_metadata_first_wins(
                        &mut seg.provider_metadata,
                        provider_metadata.as_ref(),
                    );
                }
            }

            // ─── Reasoning ───────────────────────────────────────────
            LanguageModelV4StreamPart::ReasoningStart {
                id,
                provider_metadata,
            } => {
                if self.active_reasoning.contains_key(id) {
                    trace!(id = %id, "duplicate ReasoningStart ignored");
                    return;
                }
                let idx = self.snapshot.parts.len();
                self.snapshot
                    .parts
                    .push(TurnPart::Reasoning(ReasoningSegment {
                        id: id.clone(),
                        text: String::new(),
                        provider_metadata: provider_metadata.clone(),
                    }));
                self.active_reasoning.insert(id.clone(), idx);
            }
            LanguageModelV4StreamPart::ReasoningDelta {
                id,
                delta,
                provider_metadata,
            } => {
                if let Some(&idx) = self.active_reasoning.get(id)
                    && let Some(TurnPart::Reasoning(seg)) = self.snapshot.parts.get_mut(idx)
                {
                    seg.text.push_str(delta);
                    Self::merge_metadata_first_wins(
                        &mut seg.provider_metadata,
                        provider_metadata.as_ref(),
                    );
                }
            }
            LanguageModelV4StreamPart::ReasoningEnd {
                id,
                provider_metadata,
            } => {
                if let Some(idx) = self.active_reasoning.remove(id)
                    && let Some(TurnPart::Reasoning(seg)) = self.snapshot.parts.get_mut(idx)
                {
                    // Anthropic ships its `signature` on `ReasoningEnd`,
                    // not `ReasoningStart`. Always prefer end-time
                    // metadata when present (it's the canonical close).
                    if let Some(m) = provider_metadata.as_ref() {
                        seg.provider_metadata = Some(m.clone());
                    }
                }
            }

            // ─── Tool input ──────────────────────────────────────────
            LanguageModelV4StreamPart::ToolInputStart {
                id,
                tool_name,
                provider_executed,
                dynamic,
                provider_metadata,
                ..
            } => {
                if self.active_tool.contains_key(id) {
                    trace!(id = %id, "duplicate ToolInputStart ignored");
                    return;
                }
                let idx = self.snapshot.parts.len();
                self.snapshot
                    .parts
                    .push(TurnPart::ToolCall(ToolCallSegment {
                        id: id.clone(),
                        tool_name: tool_name.clone(),
                        input_json: String::new(),
                        provider_executed: *provider_executed,
                        dynamic: *dynamic,
                        is_input_complete: false,
                        is_complete: false,
                        provider_metadata: provider_metadata.clone(),
                        invalid: false,
                        invalid_reason: None,
                    }));
                self.active_tool.insert(id.clone(), idx);
            }
            LanguageModelV4StreamPart::ToolInputDelta {
                id,
                delta,
                provider_metadata,
            } => {
                if let Some(&idx) = self.active_tool.get(id)
                    && let Some(TurnPart::ToolCall(seg)) = self.snapshot.parts.get_mut(idx)
                {
                    seg.input_json.push_str(delta);
                    Self::merge_metadata_first_wins(
                        &mut seg.provider_metadata,
                        provider_metadata.as_ref(),
                    );
                }
            }
            LanguageModelV4StreamPart::ToolInputEnd {
                id,
                provider_metadata,
            } => {
                if let Some(&idx) = self.active_tool.get(id)
                    && let Some(TurnPart::ToolCall(seg)) = self.snapshot.parts.get_mut(idx)
                {
                    seg.is_input_complete = true;
                    Self::merge_metadata_first_wins(
                        &mut seg.provider_metadata,
                        provider_metadata.as_ref(),
                    );
                }
                // Do NOT remove from active_tool yet — `ToolCall(tc)`
                // close may still come and needs to find the segment.
            }
            LanguageModelV4StreamPart::ToolCall(tc) => {
                // `LanguageModelV4ToolCall.input` is `String` (stringified
                // JSON), per `vercel-ai/provider/src/language_model/v4/tool_call.rs:20`.
                let input_str = tc.input.clone();
                if let Some(&idx) = self.active_tool.get(&tc.tool_call_id)
                    && let Some(TurnPart::ToolCall(seg)) = self.snapshot.parts.get_mut(idx)
                {
                    seg.is_complete = true;
                    // Canonical input from close. Overwrite when non-empty
                    // (deltas may have accumulated junk that the close
                    // fixes). Empty close input keeps accumulated deltas.
                    if !input_str.is_empty() {
                        seg.input_json = input_str;
                    }
                    // A4: None close MUST NOT overwrite earlier Some.
                    if tc.provider_metadata.is_some() {
                        seg.provider_metadata = tc.provider_metadata.clone();
                    }
                    if tc.provider_executed.is_some() {
                        seg.provider_executed = tc.provider_executed;
                    }
                    if tc.dynamic.is_some() {
                        seg.dynamic = tc.dynamic;
                    }
                    // Carry wire-parsing invalid_reason from the wire
                    // close into the accumulator.
                    if tc.invalid {
                        seg.invalid = true;
                        seg.invalid_reason = tc.invalid_reason.clone();
                    }
                    self.active_tool.remove(&tc.tool_call_id);
                } else {
                    // ToolCall arrived without prior ToolInputStart —
                    // push directly. Some providers (mocks too) take
                    // this shortcut.
                    self.snapshot
                        .parts
                        .push(TurnPart::ToolCall(ToolCallSegment {
                            id: tc.tool_call_id.clone(),
                            tool_name: tc.tool_name.clone(),
                            input_json: input_str,
                            provider_executed: tc.provider_executed,
                            dynamic: tc.dynamic,
                            is_input_complete: true,
                            is_complete: true,
                            provider_metadata: tc.provider_metadata.clone(),
                            invalid: tc.invalid,
                            invalid_reason: tc.invalid_reason.clone(),
                        }));
                }
            }

            // ─── File ────────────────────────────────────────────────
            LanguageModelV4StreamPart::File(file) => {
                // Stream `File.data: String` is undifferentiated (could
                // be base64 or URL by convention but no discriminator on
                // the wire). We always wrap as `Data { Base64 }` —
                // matches the dominant case (most providers emit
                // base64 for generated images); URL-bearing flows would
                // need a separate wire shape anyway.
                self.snapshot.parts.push(TurnPart::File(FileSegment {
                    id: vercel_ai_provider_utils::generate_id("file"),
                    data: LanguageModelV4FileData::Data {
                        data: vercel_ai_provider::FileRawData::Base64(file.data.clone()),
                    },
                    media_type: file.media_type.clone(),
                    provider_metadata: file.provider_metadata.clone(),
                }));
            }
            LanguageModelV4StreamPart::ReasoningFile(rfile) => {
                self.snapshot
                    .parts
                    .push(TurnPart::ReasoningFile(ReasoningFileSegment {
                        id: vercel_ai_provider_utils::generate_id("rfile"),
                        data: LanguageModelV4FileData::Data {
                            data: vercel_ai_provider::FileRawData::Base64(rfile.data.clone()),
                        },
                        media_type: rfile.media_type.clone(),
                        provider_metadata: rfile.provider_metadata.clone(),
                    }));
            }

            // ─── Source ──────────────────────────────────────────────
            LanguageModelV4StreamPart::Source(src) => {
                self.snapshot.parts.push(TurnPart::Source(SourceSegment {
                    id: src.id.clone(),
                    url: src.url.clone(),
                    title: src.title.clone(),
                    source_type: format!("{:?}", src.source_type),
                    provider_metadata: src.provider_metadata.clone(),
                }));
            }

            // ─── Custom ──────────────────────────────────────────────
            LanguageModelV4StreamPart::Custom {
                kind,
                provider_metadata,
            } => {
                self.snapshot.parts.push(TurnPart::Custom(CustomSegment {
                    id: kind.clone(),
                    data: serde_json::Value::Null,
                    provider_metadata: provider_metadata.clone(),
                }));
            }

            // ─── Tool approval request ───────────────────────────────
            LanguageModelV4StreamPart::ToolApprovalRequest(req) => {
                self.snapshot.parts.push(TurnPart::ToolApprovalRequest(
                    ToolApprovalRequestSegment {
                        approval_id: req.approval_id.clone(),
                        tool_call_id: req.tool_call_id.clone(),
                        provider_metadata: req.provider_metadata.clone(),
                    },
                ));
            }

            // ─── Non-content / lifecycle / unhandled ─────────────────
            //
            // StreamStart / Finish / Error / ResponseMetadata / Raw /
            // ToolResult / ToolInputAvailable / ToolApprovalResponse —
            // none belong in the assistant content vector. Drop.
            _ => {}
        }
    }
}

/// Events emitted during streaming inference.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// Text delta (incremental text output).
    TextDelta { text: String },
    /// Reasoning/thinking delta.
    ReasoningDelta { text: String },
    /// Reasoning/thinking block ended. Carries the provider-supplied
    /// metadata (notably `anthropic.signature` for the Anthropic-shape
    /// API and any compatible providers like DeepSeek's `/anthropic/v1`
    /// endpoint). The agent loop must thread this back into the
    /// `ReasoningPart.provider_metadata` of the assistant message it
    /// records, otherwise the next request will drop the thinking
    /// block and the API will reject it with `content[].thinking must
    /// be passed back`. `None` for providers that don't ship metadata.
    ReasoningEnd {
        provider_metadata: Option<coco_llm_types::ProviderMetadata>,
    },
    /// Tool call started.
    ToolCallStart { id: String, tool_name: String },
    /// Tool call input delta (JSON fragment).
    ToolCallDelta { id: String, delta: String },
    /// Tool call input complete.
    ToolCallEnd { id: String },
    /// Stream finished — final usage and full turn snapshot.
    ///
    /// `snapshot` carries every emitted part with its `provider_metadata`
    /// intact, in emission order. Consumers rebuilding `Vec<AssistantContentPart>`
    /// for history MUST read from `snapshot.parts` to preserve per-part
    /// signatures (Gemini `thoughtSignature`, Anthropic `signature`,
    /// OpenAI `encrypted_content`). Constructed once per turn; cheap to
    /// share via Arc.
    Finish {
        usage: TokenUsage,
        /// Typed finish reason — the `{ unified, raw }`
        /// [`coco_llm_types::FinishReason`] struct set at the
        /// vercel-ai-provider seam, propagated verbatim (no decompose
        /// into bare enum + sibling raw). Match on `.unified`; read
        /// `.raw` for provider provenance. The `ContextWindowExceeded`
        /// / `StopSequence` refinements are first-class `.unified`
        /// variants.
        stop_reason: coco_llm_types::FinishReason,
        metrics: StreamMetrics,
        snapshot: Arc<AssistantTurnSnapshot>,
    },
    /// Error during streaming.
    Error {
        message: String,
        metrics: StreamMetrics,
    },
}

/// Process a vercel-ai stream into StreamEvents sent via channel.
pub async fn process_stream(
    stream: Pin<
        Box<dyn futures::Stream<Item = Result<LanguageModelV4StreamPart, AISdkError>> + Send>,
    >,
    tx: mpsc::Sender<StreamEvent>,
) {
    process_stream_with_config(stream, tx, default_process_stream_config()).await;
}

/// Default stream processor config for the inference event bridge.
///
/// Idle timeout is **disabled by default** (the original behavior — a hard
/// idle abort would risk killing a legitimately slow reasoning stream); the
/// 30s stall threshold is the soft warn. A hard idle backstop is opt-in
/// **per-provider** via `ProviderClientOptions.stream_idle_timeout_secs`,
/// applied by [`crate::client::ApiClient::with_stream_idle_timeout`] — NOT a
/// hardcoded global.
pub fn default_process_stream_config() -> StreamProcessorConfig {
    StreamProcessorConfig::default()
        .without_idle_timeout()
        .with_stall_threshold(DEFAULT_PROCESS_STREAM_STALL_THRESHOLD)
}

/// Process a vercel-ai stream into StreamEvents using explicit processor config.
pub async fn process_stream_with_config(
    stream: Pin<
        Box<dyn futures::Stream<Item = Result<LanguageModelV4StreamPart, AISdkError>> + Send>,
    >,
    tx: mpsc::Sender<StreamEvent>,
    config: StreamProcessorConfig,
) {
    let mut processor = StreamProcessor::from_stream_with_config(stream, config);
    let mut reported_stall_count = 0;
    let mut emitted_events: i64 = 0;
    // Per-turn assistant content accumulator. Walked at `Finish` to
    // produce the `Arc<AssistantTurnSnapshot>` on `StreamEvent::Finish`.
    let mut turn_state = AssistantTurnSnapshotState::new();

    while let Some(result) = processor.next().await {
        let (event, metrics) = match result {
            Ok(part) => {
                let metrics = processor.metrics();
                // Update the per-turn assistant accumulator BEFORE converting
                // to a `StreamEvent`. `StreamProcessor` is a thin metrics +
                // idle-timeout adapter and does no accumulation of its own;
                // content state is owned here so per-part `provider_metadata`
                // round-trips intact.
                turn_state.update(&part);
                (
                    stream_event_from_part(part, metrics, &mut turn_state),
                    metrics,
                )
            }
            Err(e) => {
                let metrics = processor.metrics();
                (
                    Some(StreamEvent::Error {
                        message: e.to_string(),
                        metrics,
                    }),
                    metrics,
                )
            }
        };

        if metrics.stall_count > reported_stall_count {
            reported_stall_count = metrics.stall_count;
            warn!(
                stall_count = metrics.stall_count,
                total_stall_ms = metrics.total_stall_ms,
                "streaming stall detected"
            );
        }

        let Some(event) = event else {
            continue;
        };

        // Per-chunk trace — opt-in via `coco_inference::stream=trace`. Names
        // map directly to StreamEvent variants so a tail can grep for
        // `event=text_delta`, `event=tool_call_delta`, etc.
        match &event {
            StreamEvent::TextDelta { text } => {
                trace!(event = "text_delta", chars = text.len(), "stream event")
            }
            StreamEvent::ReasoningDelta { text } => trace!(
                event = "reasoning_delta",
                chars = text.len(),
                "stream event"
            ),
            StreamEvent::ReasoningEnd { .. } => trace!(event = "reasoning_end", "stream event"),
            StreamEvent::ToolCallStart { id, tool_name } => {
                debug!(
                    event = "tool_call_start",
                    id = %id,
                    tool_name = %tool_name,
                    "stream event"
                )
            }
            StreamEvent::ToolCallDelta { id, delta } => trace!(
                event = "tool_call_delta",
                id = %id,
                chars = delta.len(),
                "stream event"
            ),
            StreamEvent::ToolCallEnd { id } => debug!(
                event = "tool_call_end",
                id = %id,
                "stream event"
            ),
            StreamEvent::Finish {
                stop_reason, usage, ..
            } => {
                debug!(
                    event = "finish",
                    stop_reason = %stop_reason,
                    tokens_in = usage.input_tokens.total,
                    tokens_out = usage.output_tokens.total,
                    emitted = emitted_events,
                    "stream event"
                );
                // Mirror the blocking path: abnormal terminations
                // (MaxTokens / ContextWindowExceeded / ContentFilter /
                // Error / Other) surface as `warn` so they're
                // discoverable without a trace-level filter.
                if stop_reason.is_abnormal() {
                    warn!(
                        event = "finish",
                        stop_reason = %stop_reason,
                        tokens_in = usage.input_tokens.total,
                        tokens_out = usage.output_tokens.total,
                        emitted = emitted_events,
                        "stream ended on non-normal stop_reason"
                    );
                }
            }
            StreamEvent::Error { message, .. } => warn!(
                event = "error",
                message = %message,
                emitted = emitted_events,
                "stream event"
            ),
        }
        emitted_events += 1;

        if tx.send(event).await.is_err() {
            break; // Receiver dropped
        }
    }
}

fn stream_event_from_part(
    part: LanguageModelV4StreamPart,
    metrics: StreamMetrics,
    turn_state: &mut AssistantTurnSnapshotState,
) -> Option<StreamEvent> {
    match part {
        LanguageModelV4StreamPart::TextDelta { delta, .. } => {
            Some(StreamEvent::TextDelta { text: delta })
        }
        LanguageModelV4StreamPart::ReasoningDelta { delta, .. } => {
            Some(StreamEvent::ReasoningDelta { text: delta })
        }
        LanguageModelV4StreamPart::ReasoningEnd {
            provider_metadata, ..
        } => Some(StreamEvent::ReasoningEnd { provider_metadata }),
        LanguageModelV4StreamPart::ToolInputStart { id, tool_name, .. } => {
            Some(StreamEvent::ToolCallStart { id, tool_name })
        }
        LanguageModelV4StreamPart::ToolInputDelta { id, delta, .. } => {
            Some(StreamEvent::ToolCallDelta { id, delta })
        }
        LanguageModelV4StreamPart::ToolInputEnd { id, .. } => {
            // Surface the (truncated) tool arguments so operators can debug
            // approval prompts, pattern errors, and unexpected paths without
            // a TRACE-level filter. The accumulated `input_json` is fully
            // populated by `turn_state.update` before this branch runs.
            if let Some(&idx) = turn_state.active_tool.get(&id)
                && let Some(TurnPart::ToolCall(seg)) = turn_state.snapshot.parts.get(idx)
            {
                debug!(
                    event = "tool_input_complete",
                    id = %id,
                    tool_name = %seg.tool_name,
                    input_chars = seg.input_json.len(),
                    input_preview = %truncate_input_preview(&seg.input_json),
                    "stream event"
                );
            }
            Some(StreamEvent::ToolCallEnd { id })
        }
        LanguageModelV4StreamPart::Finish {
            usage,
            finish_reason,
            ..
        } => {
            // Take ownership of the accumulated snapshot — this turn is done.
            let snapshot = Arc::new(std::mem::take(&mut turn_state.snapshot));
            Some(StreamEvent::Finish {
                usage: token_usage_from_provider_usage(&usage),
                stop_reason: finish_reason,
                metrics,
                snapshot,
            })
        }
        LanguageModelV4StreamPart::Error { error } => Some(StreamEvent::Error {
            message: error.message,
            metrics,
        }),
        _ => None,
    }
}

pub(crate) fn token_usage_from_provider_usage(usage: &Usage) -> TokenUsage {
    TokenUsage {
        input_tokens: coco_types::InputTokens {
            total: u64_to_i64(usage.input_tokens.total().unwrap_or(0)),
            no_cache: u64_to_i64(usage.input_tokens.no_cache().unwrap_or(0)),
            cache_read: u64_to_i64(usage.input_tokens.cache_read().unwrap_or(0)),
            cache_write: u64_to_i64(usage.input_tokens.cache_write().unwrap_or(0)),
        },
        // Reasoning + text breakdown when the provider reports them.
        // For DeepSeek V4 / GPT-5 thinking / Claude extended thinking
        // these are non-zero on every reasoning-emitting turn. `0` when
        // the provider's wire shape doesn't separate the two.
        output_tokens: coco_types::OutputTokens {
            total: u64_to_i64(usage.output_tokens.total.unwrap_or(0)),
            text: u64_to_i64(usage.output_tokens.text.unwrap_or(0)),
            reasoning: u64_to_i64(usage.output_tokens.reasoning.unwrap_or(0)),
        },
    }
}

pub(crate) fn u64_to_i64(value: u64) -> i64 {
    value.try_into().unwrap_or(i64::MAX)
}

/// Build a synthetic `LanguageModelV4StreamResult` from a fully-materialized
/// content list.
///
/// Intended for **tests**: mocks that only implement `do_generate` can delegate
/// their `do_stream` to this helper so the streaming agent loop sees the same
/// logical response. Each content block is emitted as a `start → delta → end`
/// trio (with the canonical `ToolCall(tc)` close after `ToolInputEnd` for
/// tool calls), followed by a `Finish` event carrying `usage` and
/// `finish_reason`.
///
/// **`provider_metadata` is propagated verbatim** from each source
/// `AssistantContentPart` to every emitted stream part. This is critical
/// for round-trip tests: a mock that emits a `Reasoning` with
/// `provider_metadata = Some({"anthropic":{"signature":"S1"}})` must
/// produce a stream where the corresponding `ReasoningEnd` carries the
/// same signature. Without this, mock-based tests give false confidence.
pub fn synthetic_stream_from_content(
    content: Vec<AssistantContentPart>,
    usage: Usage,
    finish_reason: FinishReason,
) -> LanguageModelV4StreamResult {
    use LanguageModelV4StreamPart as Part;
    use vercel_ai_provider::LanguageModelV4ToolCall;

    let mut parts: Vec<Result<Part, AISdkError>> = Vec::new();
    let mut seg = 0usize;
    for block in content {
        match block {
            AssistantContentPart::Reasoning(r) => {
                let id = format!("reasoning-{seg}");
                seg += 1;
                parts.push(Ok(Part::ReasoningStart {
                    id: id.clone(),
                    provider_metadata: r.provider_metadata.clone(),
                }));
                parts.push(Ok(Part::ReasoningDelta {
                    id: id.clone(),
                    delta: r.text,
                    provider_metadata: r.provider_metadata.clone(),
                }));
                parts.push(Ok(Part::ReasoningEnd {
                    id,
                    provider_metadata: r.provider_metadata,
                }));
            }
            AssistantContentPart::Text(t) => {
                let id = format!("text-{seg}");
                seg += 1;
                parts.push(Ok(Part::TextStart {
                    id: id.clone(),
                    provider_metadata: t.provider_metadata.clone(),
                }));
                parts.push(Ok(Part::TextDelta {
                    id: id.clone(),
                    delta: t.text,
                    provider_metadata: t.provider_metadata.clone(),
                }));
                parts.push(Ok(Part::TextEnd {
                    id,
                    provider_metadata: t.provider_metadata,
                }));
            }
            AssistantContentPart::ToolCall(tc) => {
                let call_id = tc.tool_call_id.clone();
                let tool_name = tc.tool_name.clone();
                let provider_executed = tc.provider_executed;
                let provider_metadata = tc.provider_metadata.clone();
                let invalid = tc.invalid;
                let invalid_reason = tc.invalid_reason.clone();
                // `ToolCallPart.input` is `JSONValue`; the wire shape
                // expects stringified JSON, mirroring the canonical
                // `LanguageModelV4ToolCall.input: String` field.
                let input_str = serde_json::to_string(&tc.input).unwrap_or_else(|_| "{}".into());
                parts.push(Ok(Part::ToolInputStart {
                    id: call_id.clone(),
                    tool_name: tool_name.clone(),
                    provider_executed,
                    dynamic: None,
                    title: None,
                    provider_metadata: provider_metadata.clone(),
                }));
                parts.push(Ok(Part::ToolInputDelta {
                    id: call_id.clone(),
                    delta: input_str.clone(),
                    provider_metadata: provider_metadata.clone(),
                }));
                parts.push(Ok(Part::ToolInputEnd {
                    id: call_id.clone(),
                    provider_metadata: provider_metadata.clone(),
                }));
                // Canonical `ToolCall(tc)` close — this is what real
                // providers (Anthropic, OpenAI Responses, Google) emit
                // after `ToolInputEnd`. Required so the accumulator
                // can mark `is_complete=true` and downstream consumers
                // that filter on `is_complete` see the tool call.
                // Coco-rs extension: carry `invalid` + `invalid_reason`
                // through the stream so adapter-detected wire-parsing
                // failures (e.g. Anthropic streaming `content_block_stop`
                // flush) survive engine reconstruction.
                let mut close = LanguageModelV4ToolCall::new(call_id, tool_name, input_str);
                close.invalid = invalid;
                close.invalid_reason = invalid_reason;
                close.provider_executed = provider_executed;
                close.provider_metadata = provider_metadata;
                parts.push(Ok(Part::ToolCall(close)));
            }
            AssistantContentPart::File(fp) => {
                // Convert `FilePart.data` (SharedV4FileData / FileRawData)
                // to the wire `String` form for synthesis. URL-only
                // file references don't roundtrip through the stream's
                // undifferentiated `data: String`.
                let data = match &fp.data {
                    coco_llm_types::SharedV4FileData::Data { data } => match data {
                        vercel_ai_provider::FileRawData::Base64(s) => s.clone(),
                        vercel_ai_provider::FileRawData::Bytes(_) => data.to_base64(),
                    },
                    coco_llm_types::SharedV4FileData::Url { .. }
                    | coco_llm_types::SharedV4FileData::Reference { .. }
                    | coco_llm_types::SharedV4FileData::Text { .. } => {
                        trace!(
                            "Non-data File variant skipped in synthetic stream (Data-only path)"
                        );
                        continue;
                    }
                };
                parts.push(Ok(Part::File(
                    vercel_ai_provider::language_model::v4::stream::File {
                        data,
                        media_type: fp.media_type.clone(),
                        provider_metadata: fp.provider_metadata,
                    },
                )));
            }
            AssistantContentPart::ReasoningFile(rfp) => {
                let data = match &rfp.data {
                    LanguageModelV4FileData::Data { data } => match data {
                        vercel_ai_provider::FileRawData::Base64(s) => s.clone(),
                        vercel_ai_provider::FileRawData::Bytes(_) => data.to_base64(),
                    },
                    LanguageModelV4FileData::Url { .. } => {
                        trace!("ReasoningFile URL skipped in synthetic stream (Data-only path)");
                        continue;
                    }
                };
                parts.push(Ok(Part::ReasoningFile(
                    vercel_ai_provider::language_model::v4::stream::ReasoningFile {
                        data,
                        media_type: rfp.media_type.clone(),
                        provider_metadata: rfp.provider_metadata,
                    },
                )));
            }
            AssistantContentPart::Source(src) => {
                parts.push(Ok(Part::Source(src)));
            }
            AssistantContentPart::ToolApprovalRequest(req) => {
                // `ToolApprovalRequestPart` (assistant content) and
                // `LanguageModelV4ToolApprovalRequest` (stream part) are
                // distinct types — bridge by copying the two required
                // ids + metadata.
                let mut srq = vercel_ai_provider::language_model::v4::tool_approval_request::LanguageModelV4ToolApprovalRequest::new(
                    req.approval_id,
                    req.tool_call_id,
                );
                if let Some(m) = req.provider_metadata {
                    srq = srq.with_metadata(m);
                }
                parts.push(Ok(Part::ToolApprovalRequest(srq)));
            }
            // ToolResult / Custom are provider-specific and don't have
            // a clean synthetic representation — skip with a trace.
            _ => {
                trace!("AssistantContentPart variant skipped in synthetic stream");
            }
        }
    }
    parts.push(Ok(Part::Finish {
        usage,
        finish_reason,
        provider_metadata: None,
    }));

    let stream = futures::stream::iter(parts);
    LanguageModelV4StreamResult::new(Box::pin(stream))
}

/// Build a stream that opens cleanly (HTTP 200) and then yields a single
/// mid-stream `Error` part with no content before it — the shape an
/// OpenAI-compatible gateway produces when it delivers a 429 as an
/// in-stream SSE `error` event instead of an HTTP status. Test helper for
/// the engine's mid-stream recovery path; `is_retryable` is left at the
/// `StreamError::new` default (`false`) because the consume path
/// re-classifies from the message string, not this flag.
pub fn synthetic_error_stream(message: impl Into<String>) -> LanguageModelV4StreamResult {
    let part: Result<LanguageModelV4StreamPart, AISdkError> =
        Ok(LanguageModelV4StreamPart::Error {
            error: vercel_ai_provider::StreamError::new(message),
        });
    LanguageModelV4StreamResult::new(Box::pin(futures::stream::iter(vec![part])))
}

/// Like [`synthetic_error_stream`] but streams one text delta before the
/// error, so the consume path records committed output before failing.
/// Test helper for the `had_output == true` branch of the engine's
/// mid-stream recovery — once visible output has been emitted, in-place
/// retry must NOT fire (it would duplicate the output).
pub fn synthetic_error_stream_after_text(
    text: impl Into<String>,
    message: impl Into<String>,
) -> LanguageModelV4StreamResult {
    use LanguageModelV4StreamPart as Part;
    let id = "text-0".to_string();
    let parts: Vec<Result<Part, AISdkError>> = vec![
        Ok(Part::TextStart {
            id: id.clone(),
            provider_metadata: None,
        }),
        Ok(Part::TextDelta {
            id: id.clone(),
            delta: text.into(),
            provider_metadata: None,
        }),
        Ok(Part::TextEnd {
            id,
            provider_metadata: None,
        }),
        Ok(Part::Error {
            error: vercel_ai_provider::StreamError::new(message),
        }),
    ];
    LanguageModelV4StreamResult::new(Box::pin(futures::stream::iter(parts)))
}

/// Truncate accumulated tool-call JSON to a single-line debug preview.
/// UTF-8 safe; replaces interior newlines with spaces so the preview
/// doesn't break compact log formatters.
fn truncate_input_preview(input: &str) -> String {
    const MAX: usize = 160;
    let mut s: String = input.chars().take(MAX).collect();
    if input.chars().count() > MAX {
        s.push_str("...");
    }
    s.replace('\n', " ")
}

#[cfg(test)]
#[path = "stream.test.rs"]
mod tests;
