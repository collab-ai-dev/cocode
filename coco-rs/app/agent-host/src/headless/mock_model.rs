use super::*;

// ─── Mock model (no-credentials fallback) ────────────────────────────

/// Built-in mock model for development/testing.
pub struct MockModel {
    call_count: AtomicI32,
}

impl MockModel {
    pub fn new() -> Self {
        Self {
            call_count: AtomicI32::new(0),
        }
    }
}

impl Default for MockModel {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl LanguageModel for MockModel {
    fn provider(&self) -> &str {
        "mock"
    }
    fn model_id(&self) -> &str {
        "mock-model"
    }
    async fn do_generate(
        &self,
        options: &LanguageModelCallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> std::result::Result<LanguageModelGenerateResult, AISdkError> {
        let call = self.call_count.fetch_add(1, Ordering::SeqCst);
        let user_text: String = options
            .prompt
            .iter()
            .filter_map(|msg| match msg {
                coco_llm_types::LlmMessage::User { content, .. } => Some(
                    content
                        .iter()
                        .filter_map(|c| match c {
                            coco_llm_types::UserContentPart::Text(t) => Some(t.text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ");

        let response = format!(
            "[mock model, call #{call}] Received: \"{user_text}\"\n\n\
             No model configured. Set models.main via settings.json or --models.main to use a real provider."
        );

        Ok(LanguageModelGenerateResult {
            content: vec![AssistantContentPart::Text(TextPart {
                text: response,
                provider_metadata: None,
            })],
            usage: Usage::new(user_text.len() as u64 / 4, 50),
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
        abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> std::result::Result<LanguageModelStreamResult, AISdkError> {
        // Compose `do_generate` output into a synthetic stream so the
        // QueryEngine streaming path (which always calls `query_stream`)
        // works against the mock.
        let result = self.do_generate(options, abort_signal).await?;
        Ok(coco_inference::synthetic_stream_from_content(
            result.content,
            result.usage,
            result.finish_reason,
        ))
    }
}
