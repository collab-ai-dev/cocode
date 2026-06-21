//! `ScriptedModel` — a deterministic [`LanguageModel`] for hermetic tests.
//!
//! Wraps a queue of pre-built `LanguageModelGenerateResult`s. Each
//! `do_generate` / `do_stream` call pops the next response. Streaming uses
//! `synthetic_stream_from_content` so the engine sees the same per-block
//! TextStart / ToolInputStart / Finish wire shape a real provider would
//! emit. Past-end calls return a clean "stop" so the engine can finalize
//! the turn instead of hanging on a missing reply.
//!
//! Two scripting modes:
//! - `Reply::text("…")` — plain text turn (model returns + stops).
//! - `Reply::tool_call(name, input_json)` — emit a tool call. The agent
//!   loop will execute the tool and call the model again with the result;
//!   the next `Reply` in the queue answers that follow-up turn.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use coco_inference::AISdkError;
use coco_inference::LanguageModel;
use coco_inference::LanguageModelCallOptions;
use coco_inference::LanguageModelGenerateResult;
use coco_inference::LanguageModelStreamResult;
use coco_inference::synthetic_stream_from_content;
use coco_llm_types::AssistantContentPart;
use coco_llm_types::FinishReason;
use coco_llm_types::StopReason;
use coco_llm_types::TextPart;
use coco_llm_types::ToolCallPart;
use coco_llm_types::Usage;

/// One scripted assistant reply. Built via the `Reply::text` /
/// `Reply::tool_call` / `Reply::mixed` / `Reply::stop` constructors so
/// callers don't have to spell out the vercel-ai content-part shape.
#[derive(Debug, Clone)]
pub struct Reply {
    pub blocks: Vec<AssistantContentPart>,
    pub finish: StopReason,
    /// Optional per-turn token usage. `None` ⇒ a tiny non-zero default.
    /// Set via [`Reply::with_usage`] to drive token-pressure paths
    /// (e.g. the auto-compact threshold) deterministically.
    pub usage: Option<Usage>,
}

impl Reply {
    /// Plain text turn. The engine treats `Stop` as "no more turns,
    /// finalize" — used for the conversational answer the user sees.
    pub fn text(body: impl Into<String>) -> Self {
        Self {
            blocks: vec![AssistantContentPart::Text(TextPart::new(body))],
            finish: StopReason::EndTurn,
            usage: None,
        }
    }

    /// Reasoning + final text. Useful for verifying the TUI renders
    /// thinking blocks alongside the user-visible reply.
    pub fn text_with_thinking(thinking: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            blocks: vec![
                AssistantContentPart::reasoning(thinking),
                AssistantContentPart::Text(TextPart::new(body)),
            ],
            finish: StopReason::EndTurn,
            usage: None,
        }
    }

    /// Single tool call. `ToolCalls` finish-reason tells the engine to
    /// dispatch the tool and re-enter the loop with the tool result.
    pub fn tool_call(
        call_id: impl Into<String>,
        tool_name: impl Into<String>,
        input: serde_json::Value,
    ) -> Self {
        Self {
            blocks: vec![AssistantContentPart::ToolCall(ToolCallPart::new(
                call_id, tool_name, input,
            ))],
            finish: StopReason::ToolUse,
            usage: None,
        }
    }

    /// Multi-tool turn — emit several `ToolCall` blocks at once. Mirrors the
    /// shape a frontier model uses when it batches independent tool calls
    /// into a single assistant message; the agent loop's
    /// `ToolExecutor` then dispatches them concurrently or in a
    /// queue depending on each tool's `is_safe_concurrent`.
    pub fn tools<I, S1, S2>(calls: I) -> Self
    where
        I: IntoIterator<Item = (S1, S2, serde_json::Value)>,
        S1: Into<String>,
        S2: Into<String>,
    {
        let blocks = calls
            .into_iter()
            .map(|(id, name, input)| {
                AssistantContentPart::ToolCall(ToolCallPart::new(id, name, input))
            })
            .collect();
        Self {
            blocks,
            finish: StopReason::ToolUse,
            usage: None,
        }
    }

    /// Text preface + a tool call (matches what most chat models actually
    /// emit). Engine still loops because finish-reason is `ToolCalls`.
    pub fn text_then_tool(
        preface: impl Into<String>,
        call_id: impl Into<String>,
        tool_name: impl Into<String>,
        input: serde_json::Value,
    ) -> Self {
        Self {
            blocks: vec![
                AssistantContentPart::Text(TextPart::new(preface)),
                AssistantContentPart::ToolCall(ToolCallPart::new(call_id, tool_name, input)),
            ],
            finish: StopReason::ToolUse,
            usage: None,
        }
    }

    /// Empty stop. Used as a safe default when the model is exhausted —
    /// keeps the engine from hanging if a test underspecifies replies.
    pub fn stop() -> Self {
        Self {
            blocks: Vec::new(),
            finish: StopReason::EndTurn,
            usage: None,
        }
    }

    /// Attach an explicit per-turn token usage to this reply. Lets a
    /// scripted turn report a large `input_total` so the engine's
    /// post-turn `tokens_with_last_usage()` crosses the auto-compact
    /// threshold — the seam the offline auto-compact-trigger test drives.
    /// Without this, `build_result` falls back to a tiny non-zero usage.
    pub fn with_usage(mut self, input_total: u64, output_total: u64) -> Self {
        self.usage = Some(Usage::new(input_total, output_total));
        self
    }
}

/// Deterministic [`LanguageModel`] driven by a FIFO queue of [`Reply`]s.
///
/// Each `do_generate` / `do_stream` consumes one entry. When the queue is
/// empty, the model returns [`Reply::stop`] so the engine completes
/// cleanly — failing loud here would mask the test's actual assertion.
pub struct ScriptedModel {
    queue: Mutex<VecDeque<Reply>>,
    /// Total `do_generate`/`do_stream` calls observed. Lets tests assert
    /// "engine made N calls" without parsing event streams.
    calls: AtomicUsize,
}

impl ScriptedModel {
    pub fn new(replies: impl IntoIterator<Item = Reply>) -> Arc<Self> {
        Arc::new(Self {
            queue: Mutex::new(replies.into_iter().collect()),
            calls: AtomicUsize::new(0),
        })
    }

    pub fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn next_reply(&self) -> Reply {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.queue
            .lock()
            .expect("scripted-model mutex poisoned")
            .pop_front()
            .unwrap_or_else(Reply::stop)
    }

    fn build_result(&self) -> LanguageModelGenerateResult {
        let reply = self.next_reply();
        // Per-turn usage when the reply set one (see `Reply::with_usage`);
        // otherwise a tiny non-zero usage so the cost tracker /
        // TurnCompleted path doesn't take a "looks like an empty turn"
        // early-exit branch.
        let usage = reply.usage.clone().unwrap_or_else(|| Usage::new(8, 4));
        LanguageModelGenerateResult {
            content: reply.blocks,
            usage,
            finish_reason: FinishReason::new(reply.finish),
            warnings: Vec::new(),
            provider_metadata: None,
            request: None,
            response: None,
        }
    }
}

#[async_trait]
impl LanguageModel for ScriptedModel {
    fn provider(&self) -> &str {
        "scripted"
    }

    fn model_id(&self) -> &str {
        "scripted-model"
    }

    async fn do_generate(
        &self,
        _options: &LanguageModelCallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelGenerateResult, AISdkError> {
        Ok(self.build_result())
    }

    async fn do_stream(
        &self,
        _options: &LanguageModelCallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelStreamResult, AISdkError> {
        let result = self.build_result();
        Ok(synthetic_stream_from_content(
            result.content,
            result.usage,
            result.finish_reason,
        ))
    }
}
