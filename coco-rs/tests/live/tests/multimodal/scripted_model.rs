//! Capturing scripted [`LanguageModel`] for multimodal integration
//! tests.
//!
//! Mirrors `tui/scripted_model.rs::ScriptedModel` but records each
//! incoming `LanguageModelCallOptions.prompt` so tests can assert on
//! the **converted** prompt the engine assembled — i.e. the post-tool
//! prompt that carries the `LlmMessage::Tool` with
//! `ToolResultContent::Content` (multimodal payload).
//!
//! Driving the real agent loop with this model lets tests verify the
//! full upstream pipeline (`Tool::execute` → `render_for_model` →
//! `create_tool_result_message_with_parts` → `normalize_messages_for_api`)
//! without mocking the tool layer. Provider-specific wire shapes are
//! tested in each provider crate's own unit tests.

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
use coco_llm_types::LlmMessage;
use coco_llm_types::StopReason;
use coco_llm_types::TextPart;
use coco_llm_types::ToolCallPart;
use coco_llm_types::Usage;

/// One scripted assistant reply. Same shape as `tui::scripted_model::Reply`
/// but kept local so the multimodal suite doesn't depend on the
/// tui-only test module.
#[derive(Debug, Clone)]
pub struct Reply {
    pub blocks: Vec<AssistantContentPart>,
    pub finish: StopReason,
}

impl Reply {
    /// Plain-text reply that ends the turn.
    pub fn text(body: impl Into<String>) -> Self {
        Self {
            blocks: vec![AssistantContentPart::Text(TextPart::new(body))],
            finish: StopReason::EndTurn,
        }
    }

    /// Single tool call. Engine continues into the next turn after the
    /// tool executes; the next [`Reply`] in the queue answers there.
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
        }
    }

    /// Empty stop — guards under-specified queues from hanging the loop.
    pub fn stop() -> Self {
        Self {
            blocks: Vec::new(),
            finish: StopReason::EndTurn,
        }
    }
}

/// Deterministic [`LanguageModel`] that snapshots each call's prompt so
/// tests can read back the engine-assembled `LlmMessage`
/// sequence.
pub struct CapturingScriptedModel {
    queue: Mutex<VecDeque<Reply>>,
    captured_prompts: Mutex<Vec<Vec<LlmMessage>>>,
    calls: AtomicUsize,
}

impl CapturingScriptedModel {
    pub fn new(replies: impl IntoIterator<Item = Reply>) -> Arc<Self> {
        Arc::new(Self {
            queue: Mutex::new(replies.into_iter().collect()),
            captured_prompts: Mutex::new(Vec::new()),
            calls: AtomicUsize::new(0),
        })
    }

    pub fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// Snapshot of every prompt the engine sent us, in call order.
    /// Vec semantics: `[0]` = turn 1, `[1]` = turn 2, etc. Callers
    /// inspect the post-tool turn (`[1]`) to verify the tool result
    /// rode the right multimodal shape.
    pub fn captured_prompts(&self) -> Vec<Vec<LlmMessage>> {
        self.captured_prompts
            .lock()
            .expect("captured-prompts mutex poisoned")
            .clone()
    }

    fn record_and_build(&self, options: &LanguageModelCallOptions) -> LanguageModelGenerateResult {
        self.captured_prompts
            .lock()
            .expect("captured-prompts mutex poisoned")
            .push(options.prompt.clone());
        self.calls.fetch_add(1, Ordering::SeqCst);

        let reply = self
            .queue
            .lock()
            .expect("scripted-queue mutex poisoned")
            .pop_front()
            .unwrap_or_else(Reply::stop);

        // Tiny non-zero usage so cost-tracker / TurnCompleted paths
        // don't hit "looks like an empty turn" branches.
        let usage = Usage::new(8, 4);
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
impl LanguageModel for CapturingScriptedModel {
    fn provider(&self) -> &str {
        "scripted-multimodal"
    }

    fn model_id(&self) -> &str {
        "scripted-multimodal-model"
    }

    async fn do_generate(
        &self,
        options: &LanguageModelCallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelGenerateResult, AISdkError> {
        Ok(self.record_and_build(options))
    }

    async fn do_stream(
        &self,
        options: &LanguageModelCallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelStreamResult, AISdkError> {
        let result = self.record_and_build(options);
        Ok(synthetic_stream_from_content(
            result.content,
            result.usage,
            result.finish_reason,
        ))
    }
}
