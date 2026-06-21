//! Concrete `SideQuery` implementation backed by `ModelRuntimeRegistry`.

use std::sync::Arc;

use async_trait::async_trait;
use coco_inference::LanguageModelFunctionTool;
use coco_inference::LanguageModelTool;
use coco_inference::LanguageModelToolChoice;
use coco_inference::ModelRuntimeQueryOutcome;
use coco_inference::ModelRuntimeRegistry;
use coco_inference::ModelRuntimeSource;
use coco_inference::QueryParams;
use coco_inference::ResponseFormat;
use coco_messages::AssistantContent;
use coco_messages::LlmMessage;
use coco_tool_runtime::SideQuery;
use coco_types::Capability;
use coco_types::ModelRole;
use coco_types::ProviderModelSelection;
use coco_types::SideQueryRequest;
use coco_types::SideQueryResponse;
use coco_types::SideQueryRole;
use coco_types::SideQueryStopReason;
use coco_types::SideQueryToolUse;
use coco_types::SideQueryUsage;

/// `SideQuery` adapter that resolves every call through the session's
/// model runtime registry. `model_role` takes precedence; otherwise a
/// `provider/model` request uses an explicit primary-only runtime.
pub struct SideQueryAdapter {
    model_runtimes: Arc<ModelRuntimeRegistry>,
    default_model_id: String,
}

impl SideQueryAdapter {
    pub fn new(model_runtimes: Arc<ModelRuntimeRegistry>, default_model_id: String) -> Self {
        Self {
            model_runtimes,
            default_model_id,
        }
    }

    fn resolve_source(request: &SideQueryRequest) -> ModelRuntimeSource {
        if let Some(role) = request.model_role {
            return ModelRuntimeSource::Role(role);
        }
        if let Some(selection) = request
            .model
            .as_deref()
            .and_then(|raw| ProviderModelSelection::from_slash_str(raw).ok())
        {
            return ModelRuntimeSource::Explicit(selection);
        }
        ModelRuntimeSource::Role(ModelRole::Main)
    }
}

impl std::fmt::Debug for SideQueryAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SideQueryAdapter").finish_non_exhaustive()
    }
}

fn forced_tool_choice(request: &SideQueryRequest) -> Option<LanguageModelToolChoice> {
    request
        .forced_tool
        .as_ref()
        .map(|name| LanguageModelToolChoice::tool(name.clone()))
}

fn build_query_params(request: &SideQueryRequest) -> QueryParams {
    let mut prompt: Vec<LlmMessage> = Vec::with_capacity(request.messages.len() + 1);
    if !request.system.is_empty() {
        prompt.push(LlmMessage::system(&request.system));
    }
    for m in &request.messages {
        match m.role {
            SideQueryRole::User => prompt.push(LlmMessage::user_text(&m.content)),
            SideQueryRole::Assistant => prompt.push(LlmMessage::assistant_text(&m.content)),
        }
    }

    let tools = if request.tools.is_empty() {
        None
    } else {
        Some(
            request
                .tools
                .iter()
                .map(|t| {
                    LanguageModelTool::Function(LanguageModelFunctionTool {
                        name: t.name.clone(),
                        description: Some(t.description.clone()),
                        input_schema: t.input_schema.clone(),
                        input_examples: None,
                        strict: None,
                        provider_options: None,
                    })
                })
                .collect::<Vec<_>>(),
        )
    };

    let response_format = request
        .output_format
        .as_ref()
        .map(|fmt| ResponseFormat::Json {
            schema: Some(fmt.schema.clone()),
            name: fmt.name.clone(),
            description: fmt.description.clone(),
        });

    QueryParams {
        prompt,
        max_tokens: request.max_tokens.map(i64::from),
        thinking_level: None,
        fast_mode: false,
        tools,
        tool_choice: forced_tool_choice(request),
        context_management: None,
        query_source: Some(request.query_source.clone()),
        agent_id: None,
        time_since_last_assistant_ms: None,
        agentic: false,
        cache: None,
        stop_sequences: None,
        response_format,
        cancel: None,
        wire_tap: None,
    }
}

#[async_trait]
impl SideQuery for SideQueryAdapter {
    async fn query(
        &self,
        request: SideQueryRequest,
    ) -> Result<SideQueryResponse, coco_error::BoxedError> {
        let source = Self::resolve_source(&request);

        let result = loop {
            let params = build_query_params(&request);
            match self
                .model_runtimes
                .query_once(source.clone(), &params)
                .await
            {
                ModelRuntimeQueryOutcome::Success { result, .. } => break result,
                ModelRuntimeQueryOutcome::Retry { .. } => continue,
                ModelRuntimeQueryOutcome::Failed { error, .. } => {
                    return Err(Box::new(coco_error::PlainError::new(
                        error.to_string(),
                        coco_error::StatusCode::ProviderError,
                    )) as coco_error::BoxedError);
                }
            }
        };

        let mut text_buf = String::new();
        let mut tool_uses = Vec::new();
        for c in &result.content {
            match c {
                AssistantContent::Text(t) => text_buf.push_str(&t.text),
                AssistantContent::ToolCall(tc) => {
                    tool_uses.push(SideQueryToolUse {
                        name: tc.tool_name.clone(),
                        input: tc.input.clone(),
                        invalid: tc.invalid,
                    });
                }
                _ => {}
            }
        }

        use coco_llm_types::StopReason;
        let stop_reason = match result.stop_reason.as_ref() {
            None => SideQueryStopReason::EndTurn,
            Some(f) => match f.unified {
                StopReason::EndTurn => SideQueryStopReason::EndTurn,
                StopReason::StopSequence => SideQueryStopReason::StopSequence,
                StopReason::ToolUse => SideQueryStopReason::ToolUse,
                StopReason::MaxTokens | StopReason::ContextWindowExceeded => {
                    SideQueryStopReason::MaxTokens
                }
                StopReason::ContentFilter | StopReason::Error | StopReason::Other => {
                    let raw = f
                        .raw
                        .clone()
                        .unwrap_or_else(|| f.unified.as_wire_str().to_string());
                    SideQueryStopReason::Other(raw)
                }
            },
        };

        Ok(SideQueryResponse {
            text: if text_buf.is_empty() {
                None
            } else {
                Some(text_buf)
            },
            tool_uses,
            stop_reason,
            usage: SideQueryUsage {
                input_tokens: result.usage.input_tokens.total,
                output_tokens: result.usage.output_tokens.total,
            },
            model_used: result.model,
        })
    }

    fn model_id(&self) -> &str {
        &self.default_model_id
    }

    fn supports_capability(&self, role: Option<ModelRole>, capability: Capability) -> bool {
        let role = role.unwrap_or(ModelRole::Main);
        let Ok(snapshot) = self.model_runtimes.snapshot_for_role(role) else {
            return false;
        };
        snapshot
            .model_info
            .and_then(|info| info.capabilities)
            .as_deref()
            .unwrap_or(&[])
            .contains(&capability)
    }
}

#[cfg(test)]
#[path = "side_query_impl.test.rs"]
mod tests;
