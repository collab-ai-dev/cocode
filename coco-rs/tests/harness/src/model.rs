use std::sync::Arc;
use std::sync::atomic::AtomicI32;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use coco_inference::AISdkError;
use coco_inference::LanguageModel;
use coco_inference::LanguageModelCallOptions;
use coco_inference::LanguageModelGenerateResult;
use coco_inference::LanguageModelStreamResult;
use coco_llm_types::AssistantContentPart;
use coco_llm_types::FinishReason;
use coco_llm_types::StopReason;
use coco_llm_types::TextPart;
use coco_llm_types::ToolCallPart;
use coco_llm_types::Usage;

fn parse_raw_arguments_like_adapter(raw: &str) -> serde_json::Value {
    use coco_utils_json_repair::RepairOutcome;
    use coco_utils_json_repair::parse_with_repair;

    if raw.trim().is_empty() {
        return serde_json::Value::Object(serde_json::Map::new());
    }
    match parse_with_repair(raw) {
        Ok((value, RepairOutcome::Clean | RepairOutcome::Repaired)) => value,
        Err(_) => serde_json::Value::String(raw.to_string()),
    }
}

pub enum MockResponse {
    Text(String),
    ToolCall {
        tool_name: String,
        input: serde_json::Value,
    },
    TextAndToolCalls {
        text: String,
        tool_calls: Vec<(String, serde_json::Value)>,
    },
    MultiToolCall(Vec<(String, serde_json::Value)>),
    MixedToolCalls(Vec<MockToolEmission>),
}

pub enum MockToolEmission {
    Clean {
        tool_name: String,
        input: serde_json::Value,
    },
    FromRawArguments {
        tool_name: String,
        raw_arguments: String,
    },
    InvalidWithReason {
        tool_name: String,
        input: serde_json::Value,
        reason: coco_llm_types::ToolInputInvalidReason,
    },
}

impl MockToolEmission {
    pub fn clean(tool_name: &str, input: serde_json::Value) -> Self {
        Self::Clean {
            tool_name: tool_name.to_string(),
            input,
        }
    }

    pub fn from_raw(tool_name: &str, raw_arguments: &str) -> Self {
        Self::FromRawArguments {
            tool_name: tool_name.to_string(),
            raw_arguments: raw_arguments.to_string(),
        }
    }

    pub fn invalid(
        tool_name: &str,
        input: serde_json::Value,
        reason: coco_llm_types::ToolInputInvalidReason,
    ) -> Self {
        Self::InvalidWithReason {
            tool_name: tool_name.to_string(),
            input,
            reason,
        }
    }

    fn into_part(self, idx: usize, call_idx: i32) -> AssistantContentPart {
        let tool_call_id = format!("call_{call_idx}_{idx}");
        match self {
            Self::Clean { tool_name, input } => AssistantContentPart::ToolCall(ToolCallPart {
                tool_call_id,
                tool_name,
                input,
                provider_executed: None,
                provider_metadata: None,
                invalid: false,
                invalid_reason: None,
            }),
            Self::FromRawArguments {
                tool_name,
                raw_arguments,
            } => AssistantContentPart::ToolCall(ToolCallPart {
                tool_call_id,
                tool_name,
                input: parse_raw_arguments_like_adapter(&raw_arguments),
                provider_executed: None,
                provider_metadata: None,
                invalid: false,
                invalid_reason: None,
            }),
            Self::InvalidWithReason {
                tool_name,
                input,
                reason,
            } => AssistantContentPart::ToolCall(ToolCallPart {
                tool_call_id,
                tool_name,
                input,
                provider_executed: None,
                provider_metadata: None,
                invalid: true,
                invalid_reason: Some(reason),
            }),
        }
    }
}

impl MockResponse {
    pub fn text(s: &str) -> Self {
        Self::Text(s.to_string())
    }

    pub fn tool_call(name: &str, input: serde_json::Value) -> Self {
        Self::ToolCall {
            tool_name: name.to_string(),
            input,
        }
    }

    pub fn multi_tool(calls: Vec<(&str, serde_json::Value)>) -> Self {
        Self::MultiToolCall(calls.into_iter().map(|(n, i)| (n.to_string(), i)).collect())
    }

    fn into_generate_result(self, call_idx: i32) -> LanguageModelGenerateResult {
        let (content, finish) = match self {
            Self::Text(text) => (
                vec![AssistantContentPart::Text(TextPart {
                    text,
                    provider_metadata: None,
                })],
                StopReason::EndTurn,
            ),
            Self::ToolCall { tool_name, input } => (
                vec![AssistantContentPart::ToolCall(ToolCallPart {
                    tool_call_id: format!("call_{call_idx}"),
                    tool_name,
                    input,
                    provider_executed: None,
                    provider_metadata: None,
                    invalid: false,
                    invalid_reason: None,
                })],
                StopReason::ToolUse,
            ),
            Self::TextAndToolCalls { text, tool_calls } => {
                let mut parts = vec![AssistantContentPart::Text(TextPart {
                    text,
                    provider_metadata: None,
                })];
                for (i, (tool_name, input)) in tool_calls.into_iter().enumerate() {
                    parts.push(AssistantContentPart::ToolCall(ToolCallPart {
                        tool_call_id: format!("call_{call_idx}_{i}"),
                        tool_name,
                        input,
                        provider_executed: None,
                        provider_metadata: None,
                        invalid: false,
                        invalid_reason: None,
                    }));
                }
                (parts, StopReason::ToolUse)
            }
            Self::MultiToolCall(calls) => {
                let parts = calls
                    .into_iter()
                    .enumerate()
                    .map(|(i, (tool_name, input))| {
                        AssistantContentPart::ToolCall(ToolCallPart {
                            tool_call_id: format!("call_{call_idx}_{i}"),
                            tool_name,
                            input,
                            provider_executed: None,
                            provider_metadata: None,
                            invalid: false,
                            invalid_reason: None,
                        })
                    })
                    .collect();
                (parts, StopReason::ToolUse)
            }
            Self::MixedToolCalls(emissions) => {
                let parts = emissions
                    .into_iter()
                    .enumerate()
                    .map(|(i, emission)| emission.into_part(i, call_idx))
                    .collect();
                (parts, StopReason::ToolUse)
            }
        };

        LanguageModelGenerateResult {
            content,
            usage: Usage::new(50, 20),
            finish_reason: FinishReason::new(finish),
            warnings: vec![],
            provider_metadata: None,
            request: None,
            response: None,
        }
    }
}

type ResponseFn = Box<dyn Fn(&LanguageModelCallOptions) -> MockResponse + Send + Sync>;

pub struct ScriptedMock {
    call_count: AtomicI32,
    responses: Vec<ResponseFn>,
}

impl ScriptedMock {
    fn get_response(&self, options: &LanguageModelCallOptions) -> MockResponse {
        let idx = self.call_count.fetch_add(1, Ordering::SeqCst) as usize;
        if idx < self.responses.len() {
            (self.responses[idx])(options)
        } else {
            MockResponse::text("(mock: no more scripted responses)")
        }
    }
}

#[async_trait]
impl LanguageModel for ScriptedMock {
    fn provider(&self) -> &str {
        "mock"
    }

    fn model_id(&self) -> &str {
        "scripted-mock"
    }

    async fn do_generate(
        &self,
        options: &LanguageModelCallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelGenerateResult, AISdkError> {
        let idx = self.call_count.load(Ordering::SeqCst);
        let response = self.get_response(options);
        Ok(response.into_generate_result(idx))
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

pub struct MockModelBuilder {
    responses: Vec<ResponseFn>,
}

impl Default for MockModelBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl MockModelBuilder {
    pub fn new() -> Self {
        Self {
            responses: Vec::new(),
        }
    }

    pub fn on_call<F>(mut self, _idx: usize, f: F) -> Self
    where
        F: Fn(&LanguageModelCallOptions) -> MockResponse + Send + Sync + 'static,
    {
        self.responses.push(Box::new(f));
        self
    }

    pub fn then_text(self, text: &str) -> Self {
        let text = text.to_string();
        self.on_call(0, move |_| MockResponse::Text(text.clone()))
    }

    pub fn then_tool_call(self, name: &str, input: serde_json::Value) -> Self {
        let name = name.to_string();
        self.on_call(0, move |_| MockResponse::ToolCall {
            tool_name: name.clone(),
            input: input.clone(),
        })
    }

    pub fn build(self) -> Arc<ScriptedMock> {
        Arc::new(ScriptedMock {
            call_count: AtomicI32::new(0),
            responses: self.responses,
        })
    }
}

#[cfg(test)]
#[path = "model.test.rs"]
mod tests;
