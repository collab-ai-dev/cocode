use std::sync::Arc;
use std::sync::Mutex;

use coco_config::MoaEndpointSpec;
use coco_config::MoaFanout;
use coco_inference::ModelRuntimeQueryOutcome;
use coco_inference::ModelRuntimeRegistry;
use coco_inference::ModelRuntimeSource;
use coco_inference::QueryParams;
use coco_llm_types::AssistantContentPart;
use coco_llm_types::LlmMessage;
use coco_llm_types::LlmPrompt;
use coco_llm_types::ToolContentPart;
use coco_llm_types::ToolResultContent;
use coco_llm_types::ToolResultContentPart;
use coco_llm_types::UserContentPart;
use coco_types::AgentStreamEvent;
use coco_types::MoaAggregatingParams;
use coco_types::MoaReferenceParams;
use coco_types::ModelRole;
use coco_types::ProviderModelSelection;
use coco_types::ServerNotification;
use coco_types::ThinkingLevel;
use coco_types::TurnId;
use futures::future::join_all;
use lru::LruCache;
use once_cell::sync::Lazy;

use crate::CoreEvent;
use crate::engine::QueryEngine;
use crate::usage_accounting::UsageAccounting;
use crate::usage_accounting::UsageRecord;

const USER_TURN_REFERENCE_CACHE_CAPACITY: std::num::NonZeroUsize =
    match std::num::NonZeroUsize::new(32) {
        Some(value) => value,
        None => std::num::NonZeroUsize::MIN,
    };

static USER_TURN_REFERENCE_CACHE: Lazy<Mutex<LruCache<String, Vec<ReferenceOutput>>>> =
    Lazy::new(|| Mutex::new(LruCache::new(USER_TURN_REFERENCE_CACHE_CAPACITY)));

const MAX_REFERENCE_QUERY_ATTEMPTS: usize = 3;
/// Default per-reference wall-clock bound. One slow advisor must not stall
/// the whole fan-out; the aggregator runs with whatever came back.
const DEFAULT_REFERENCE_TIMEOUT_SECS: u64 = 120;
const TOOL_RESULT_TEXT_BUDGET: usize = 4_000;
const REFERENCE_GUIDANCE_TEXT_BUDGET: usize = 24_000;
const REFERENCE_SYSTEM_PROMPT: &str = "\
You are a reference advisor in a Mixture of Agents process. You are not the \
acting agent and you do not execute tools, browse, run commands, or access \
files directly. A separate aggregator model will act on the task.\n\n\
Analyze the conversation state and provide concise, concrete guidance for the \
aggregator: next steps, tool-use strategy, risks, disagreements, and anything \
the acting agent may be missing. Do not apologize for lacking access.\n\n\
NEVER claim to have executed a tool, run a command, edited a file, or \
fetched a page yourself — you cannot. Phrase suggestions as recommendations \
for the acting agent (\"run X and check Y\"), never as completed actions \
(\"I ran X\").";
const ADVISORY_INSTRUCTION: &str = "\
The conversation above is the current state of the task. Give your best \
judgement about what should happen next and what risks or mistakes you see.";

pub(crate) async fn maybe_attach_moa_guidance(
    engine: &QueryEngine,
    model_runtimes: &Arc<ModelRuntimeRegistry>,
    source: &ModelRuntimeSource,
    params: &QueryParams,
    event_tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>,
    turn_id: &TurnId,
) -> QueryParams {
    let Some(endpoint) = model_runtimes.moa_endpoint_for_source(source) else {
        return params.clone();
    };
    let role = role_for_source(source);
    let references = run_references(
        MoaReferenceUsageRecorder::Engine(engine),
        model_runtimes,
        &endpoint,
        params,
        event_tx,
        turn_id,
        role,
        Some(&engine.cancel),
    )
    .await;
    attach_reference_guidance(params, &endpoint, &references)
}

#[derive(Clone, Copy)]
pub(crate) enum MoaReferenceUsageRecorder<'a> {
    Engine(&'a QueryEngine),
    Accounting(&'a UsageAccounting),
    None,
}

pub(crate) async fn maybe_attach_moa_guidance_for_query_once(
    model_runtimes: &Arc<ModelRuntimeRegistry>,
    source: &ModelRuntimeSource,
    params: &QueryParams,
    event_tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>,
    turn_id: &TurnId,
    usage_recorder: MoaReferenceUsageRecorder<'_>,
) -> QueryParams {
    let Some(endpoint) = model_runtimes.moa_endpoint_for_source(source) else {
        return params.clone();
    };
    let role = role_for_source(source);
    let references = run_references(
        usage_recorder,
        model_runtimes,
        &endpoint,
        params,
        event_tx,
        turn_id,
        role,
        /*cancel*/ None,
    )
    .await;
    attach_reference_guidance(params, &endpoint, &references)
}

pub async fn prepare_moa_query_once_params_no_usage(
    model_runtimes: &Arc<ModelRuntimeRegistry>,
    source: &ModelRuntimeSource,
    params: &QueryParams,
    turn_id: &TurnId,
) -> QueryParams {
    let event_tx = None;
    maybe_attach_moa_guidance_for_query_once(
        model_runtimes,
        source,
        params,
        &event_tx,
        turn_id,
        MoaReferenceUsageRecorder::None,
    )
    .await
}

pub async fn prepare_moa_query_once_params_with_usage_accounting(
    model_runtimes: &Arc<ModelRuntimeRegistry>,
    source: &ModelRuntimeSource,
    params: &QueryParams,
    turn_id: &TurnId,
    usage_accounting: Option<&UsageAccounting>,
) -> QueryParams {
    let event_tx = None;
    let usage_recorder = usage_accounting
        .map(MoaReferenceUsageRecorder::Accounting)
        .unwrap_or(MoaReferenceUsageRecorder::None);
    maybe_attach_moa_guidance_for_query_once(
        model_runtimes,
        source,
        params,
        &event_tx,
        turn_id,
        usage_recorder,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_references(
    usage_recorder: MoaReferenceUsageRecorder<'_>,
    model_runtimes: &Arc<ModelRuntimeRegistry>,
    endpoint: &MoaEndpointSpec,
    params: &QueryParams,
    event_tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>,
    turn_id: &TurnId,
    role: ModelRole,
    cancel: Option<&tokio_util::sync::CancellationToken>,
) -> Vec<ReferenceOutput> {
    let count = endpoint.reference_models.len();
    let prompt = reference_prompt(&params.prompt);
    let cache_key = user_turn_cache_key(endpoint, &prompt, turn_id);
    if let Some(key) = cache_key.as_ref()
        && let Some(cached) = load_reference_cache(key)
    {
        return cached;
    }
    emit_reference_started(event_tx, turn_id, role, endpoint);
    let tasks = endpoint
        .reference_models
        .iter()
        .enumerate()
        .map(|(idx, slot)| {
            let model_runtimes = Arc::clone(model_runtimes);
            let preset = endpoint.preset_name.clone();
            let provider = slot.model.provider.clone();
            let model_id = slot.model.model_id.clone();
            let selection = ProviderModelSelection {
                provider: provider.clone(),
                model_id: model_id.clone(),
            };
            // Per-advisor prompt window (N7c): a preset routinely mixes a
            // small cheap advisor with a large one, and one shared byte budget
            // either overflows the small model or wastes the large one's room.
            let prompt = fit_prompt_to_advisor(
                prompt.clone(),
                advisor_input_budget(&model_runtimes, &selection),
                &provider,
                &model_id,
            );
            let mut reference_params = params.clone();
            reference_params.prompt = prompt;
            reference_params.tools = None;
            reference_params.tool_choice = None;
            reference_params.context_management = None;
            reference_params.response_format = None;
            reference_params.agentic = false;
            reference_params.fast_mode = false;
            reference_params.stop_sequences = None;
            reference_params.fallback_min_context_window = None;
            reference_params.agent_id = None;
            reference_params.time_since_last_assistant_ms = None;
            // Temperature and max_tokens are deliberately NOT set: they are
            // properties of the advisor model, already resolved from its own
            // `ModelInfo`. A preset-wide value would flatten every advisor's
            // per-model configuration.
            //
            // Thinking does not inherit the parent turn (an advisor is not the
            // acting model), so the slot's own `effort` is its only control —
            // `None` still defers to the model's `default_thinking_level`.
            reference_params.thinking_level = slot.effort.map(|effort| ThinkingLevel {
                effort,
                budget_tokens: None,
                options: std::collections::HashMap::new(),
            });
            reference_params.query_source = Some(format!(
                "moa_reference:{preset}:{idx}:{provider}/{model_id}"
            ));
            let timeout_secs = endpoint
                .reference_timeout_secs
                .map(|v| v.max(0) as u64)
                .unwrap_or(DEFAULT_REFERENCE_TIMEOUT_SECS);
            async move {
                let started = std::time::Instant::now();
                let selection = ProviderModelSelection {
                    provider: provider.clone(),
                    model_id: model_id.clone(),
                };
                // Per-reference wall-clock bound (0 = unbounded): a stalled
                // advisor becomes a `[failed: …]` marker instead of holding
                // the aggregator hostage.
                let query =
                    query_reference_with_retry(&model_runtimes, selection, &reference_params);
                let outcome = if timeout_secs == 0 {
                    query.await
                } else {
                    match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), query)
                        .await
                    {
                        Ok(outcome) => outcome,
                        Err(_) => {
                            return ReferenceOutput {
                                index: idx,
                                count,
                                provider,
                                model_id,
                                text: String::new(),
                                failed: Some(format!("reference timed out after {timeout_secs}s")),
                                usage: None,
                            };
                        }
                    }
                };
                match outcome {
                    ModelRuntimeQueryOutcome::Success { result, .. } => ReferenceOutput {
                        index: idx,
                        count,
                        provider,
                        model_id,
                        text: assistant_text(&result.content),
                        failed: None,
                        usage: Some((result.usage, started.elapsed().as_millis() as i64)),
                    },
                    ModelRuntimeQueryOutcome::Retry { .. } => ReferenceOutput {
                        index: idx,
                        count,
                        provider,
                        model_id,
                        text: String::new(),
                        failed: Some("reference model requested retry".to_string()),
                        usage: None,
                    },
                    ModelRuntimeQueryOutcome::Failed { error, .. } => ReferenceOutput {
                        index: idx,
                        count,
                        provider,
                        model_id,
                        text: String::new(),
                        failed: Some(error.to_string()),
                        usage: None,
                    },
                }
            }
        });
    // User interrupt aborts the fan-out wait: return empty so the caller
    // attaches no guidance — the surrounding turn is being cancelled and
    // must not block on slow advisors. Nothing is cached on this path.
    let mut outputs = match cancel {
        Some(token) => {
            tokio::select! {
                outputs = join_all(tasks) => outputs,
                () = token.cancelled() => {
                    tracing::info!(
                        preset = %endpoint.preset_name,
                        "MoA reference fan-out aborted by interrupt"
                    );
                    return Vec::new();
                }
            }
        }
        None => join_all(tasks).await,
    };
    outputs.sort_by_key(|output| output.index);
    for output in &outputs {
        emit_reference_completed(event_tx, turn_id, role, endpoint, output).await;
        if let Some((usage, duration_ms)) = output.usage {
            record_reference_usage(usage_recorder, event_tx, output, usage, duration_ms).await;
        }
    }
    emit_moa_aggregating(event_tx, turn_id, role, endpoint).await;
    emit_reference_thinking_blocks(event_tx, turn_id, &outputs).await;
    if let Some(key) = cache_key {
        store_reference_cache(key, &outputs);
    }
    outputs
}

async fn record_reference_usage(
    usage_recorder: MoaReferenceUsageRecorder<'_>,
    event_tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>,
    output: &ReferenceOutput,
    usage: coco_types::TokenUsage,
    duration_ms: i64,
) {
    match usage_recorder {
        MoaReferenceUsageRecorder::Engine(engine) => {
            engine
                .record_session_usage(
                    event_tx,
                    &output.provider,
                    &output.model_id,
                    usage,
                    duration_ms,
                    coco_types::UsageSource::MoaReference,
                )
                .await;
        }
        MoaReferenceUsageRecorder::Accounting(accounting) => {
            accounting
                .record_usage(UsageRecord {
                    provider: &output.provider,
                    model_id: &output.model_id,
                    usage,
                    duration_ms,
                    source: coco_types::UsageSource::MoaReference,
                    auto_compact_threshold: None,
                    event_tx: event_tx.as_ref(),
                })
                .await;
        }
        MoaReferenceUsageRecorder::None => {}
    }
}

fn role_for_source(source: &ModelRuntimeSource) -> ModelRole {
    match source {
        ModelRuntimeSource::Role(role) => *role,
        ModelRuntimeSource::Explicit(_) => ModelRole::Main,
    }
}

fn emit_reference_started(
    event_tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>,
    turn_id: &TurnId,
    role: ModelRole,
    endpoint: &MoaEndpointSpec,
) {
    let Some(tx) = event_tx.as_ref() else {
        return;
    };
    let count = endpoint.reference_models.len() as i32;
    for (idx, slot) in endpoint.reference_models.iter().enumerate() {
        let _ = tx.try_send(CoreEvent::Protocol(
            ServerNotification::MoaReferenceStarted(MoaReferenceParams {
                turn_id: turn_id.clone(),
                role,
                preset: endpoint.preset_name.clone(),
                index: (idx + 1) as i32,
                count,
                provider: slot.model.provider.clone(),
                model_id: slot.model.model_id.clone(),
                text: String::new(),
                failed: false,
            }),
        ));
    }
}

async fn emit_reference_completed(
    event_tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>,
    turn_id: &TurnId,
    role: ModelRole,
    endpoint: &MoaEndpointSpec,
    output: &ReferenceOutput,
) {
    let Some(tx) = event_tx.as_ref() else {
        return;
    };
    let text = output
        .failed
        .as_ref()
        .map(|error| format!("[failed: {error}]"))
        .unwrap_or_else(|| output.text.clone());
    let _ = tx
        .send(CoreEvent::Protocol(
            ServerNotification::MoaReferenceCompleted(MoaReferenceParams {
                turn_id: turn_id.clone(),
                role,
                preset: endpoint.preset_name.clone(),
                index: (output.index + 1) as i32,
                count: output.count as i32,
                provider: output.provider.clone(),
                model_id: output.model_id.clone(),
                text,
                failed: output.failed.is_some(),
            }),
        ))
        .await;
}

async fn emit_moa_aggregating(
    event_tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>,
    turn_id: &TurnId,
    role: ModelRole,
    endpoint: &MoaEndpointSpec,
) {
    let Some(tx) = event_tx.as_ref() else {
        return;
    };
    let _ = tx
        .send(CoreEvent::Protocol(ServerNotification::MoaAggregating(
            MoaAggregatingParams {
                turn_id: turn_id.clone(),
                role,
                preset: endpoint.preset_name.clone(),
                count: endpoint.reference_models.len() as i32,
            },
        )))
        .await;
}

async fn emit_reference_thinking_blocks(
    event_tx: &Option<tokio::sync::mpsc::Sender<CoreEvent>>,
    turn_id: &TurnId,
    outputs: &[ReferenceOutput],
) {
    let Some(tx) = event_tx.as_ref() else {
        return;
    };
    for output in outputs {
        let body = output
            .failed
            .as_ref()
            .map(|error| format!("[failed: {error}]"))
            .unwrap_or_else(|| output.text.clone());
        let block = format!(
            "MoA reference {}/{} — {}/{}\n{}\n\n",
            output.index + 1,
            output.count,
            output.provider,
            output.model_id,
            body.trim()
        );
        let _ = tx
            .send(CoreEvent::Stream(AgentStreamEvent::ThinkingDelta {
                turn_id: turn_id.clone(),
                delta: block,
            }))
            .await;
    }
}

#[derive(Debug, Clone)]
struct ReferenceOutput {
    index: usize,
    count: usize,
    provider: String,
    model_id: String,
    text: String,
    failed: Option<String>,
    usage: Option<(coco_types::TokenUsage, i64)>,
}

impl ReferenceOutput {
    fn without_usage(mut self) -> Self {
        self.usage = None;
        self
    }
}

fn load_reference_cache(key: &str) -> Option<Vec<ReferenceOutput>> {
    let mut cache = USER_TURN_REFERENCE_CACHE.lock().ok()?;
    cache.get(key).cloned()
}

fn store_reference_cache(key: String, outputs: &[ReferenceOutput]) {
    let Ok(mut cache) = USER_TURN_REFERENCE_CACHE.lock() else {
        return;
    };
    cache.put(
        key,
        outputs
            .iter()
            .cloned()
            .map(ReferenceOutput::without_usage)
            .collect(),
    );
}

fn user_turn_cache_key(
    endpoint: &MoaEndpointSpec,
    prompt: &LlmPrompt,
    turn_id: &TurnId,
) -> Option<String> {
    if endpoint.fanout != MoaFanout::UserTurn {
        return None;
    }
    let mut last_real_user = None;
    for (idx, message) in prompt.iter().enumerate().rev() {
        if let LlmMessage::User { content, .. } = message
            && user_text(content) != ADVISORY_INSTRUCTION
        {
            last_real_user = Some(idx);
            break;
        }
    }
    let signature_prompt = last_real_user
        .map(|idx| &prompt[..=idx])
        .unwrap_or(prompt.as_slice());
    let signature_messages = signature_prompt
        .iter()
        .filter(|message| !is_reference_system_message(message))
        .collect::<Vec<_>>();
    let signature = serde_json::to_string(&signature_messages).ok()?;
    let signature_hash = cache_signature_hash(&signature);
    // Effort participates in the label: two presets differing only in an
    // advisor's effort produce different guidance and must not share an entry.
    let labels = endpoint
        .reference_models
        .iter()
        .map(|slot| match slot.effort {
            Some(effort) => format!(
                "{}/{}@{}",
                slot.model.provider,
                slot.model.model_id,
                effort.as_str()
            ),
            None => format!("{}/{}", slot.model.provider, slot.model.model_id),
        })
        .collect::<Vec<_>>()
        .join(",");
    Some(format!(
        "{}\u{0}{}\u{0}{}\u{0}{signature_hash:x}",
        turn_id.as_str(),
        endpoint.preset_name,
        labels
    ))
}

fn is_reference_system_message(message: &LlmMessage) -> bool {
    matches!(message, LlmMessage::System { content, .. } if user_text(content) == REFERENCE_SYSTEM_PROMPT)
}

fn cache_signature_hash(input: &str) -> u64 {
    use std::hash::Hash;
    use std::hash::Hasher;

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    input.hash(&mut hasher);
    hasher.finish()
}

fn reference_prompt(prompt: &LlmPrompt) -> LlmPrompt {
    let mut out = vec![LlmMessage::system(REFERENCE_SYSTEM_PROMPT)];
    for message in prompt {
        match message {
            LlmMessage::System { .. } | LlmMessage::Developer { .. } => {}
            LlmMessage::User { content, .. } => {
                let text = user_text(content);
                if !text.is_empty() {
                    out.push(LlmMessage::user_text(text));
                }
            }
            LlmMessage::Assistant { content, .. } => {
                let text = assistant_text(content);
                if !text.is_empty() {
                    out.push(LlmMessage::assistant_text(text));
                }
            }
            LlmMessage::Tool { content, .. } => {
                let text = tool_text(content);
                if !text.is_empty() {
                    push_tool_result_message(&mut out, text);
                }
            }
        }
    }
    if !matches!(out.last(), Some(LlmMessage::User { .. })) {
        out.push(LlmMessage::user_text(ADVISORY_INSTRUCTION));
    }
    out
}

async fn query_reference_with_retry(
    model_runtimes: &Arc<ModelRuntimeRegistry>,
    selection: ProviderModelSelection,
    params: &QueryParams,
) -> ModelRuntimeQueryOutcome {
    for attempt in 0..MAX_REFERENCE_QUERY_ATTEMPTS {
        match model_runtimes
            .query_once(ModelRuntimeSource::Explicit(selection.clone()), params)
            .await
        {
            ModelRuntimeQueryOutcome::Retry { .. }
                if attempt + 1 < MAX_REFERENCE_QUERY_ATTEMPTS =>
            {
                continue;
            }
            done => return done,
        }
    }
    unreachable!("bounded reference retry loop always returns on final attempt")
}

fn attach_reference_guidance(
    params: &QueryParams,
    endpoint: &MoaEndpointSpec,
    references: &[ReferenceOutput],
) -> QueryParams {
    // The aggregator IS the acting model, so it keeps the parent turn's
    // temperature and thinking verbatim — a preset-level override here would
    // silently defeat the user's live effort/thinking controls.
    let mut next = params.clone();
    let mut guidance = format!(
        "[Mixture of Agents reference context]\nPreset: {}\nAggregator/acting model: {}/{}\n\nUse these independent model notes as private advisory context only. You are the acting model: answer the user directly or call tools as needed.\n",
        endpoint.preset_name, endpoint.aggregator.provider, endpoint.aggregator.model_id
    );
    for reference in references {
        guidance.push_str(&format!(
            "\n[reference {}/{}: {}/{}]\n",
            reference.index + 1,
            reference.count,
            reference.provider,
            reference.model_id
        ));
        if let Some(error) = &reference.failed {
            guidance.push_str("[failed: ");
            guidance.push_str(&truncate_head_tail(error, TOOL_RESULT_TEXT_BUDGET));
            guidance.push_str("]\n");
        } else if reference.text.trim().is_empty() {
            guidance.push_str("[empty]\n");
        } else {
            guidance.push_str(&truncate_head_tail(
                reference.text.trim(),
                REFERENCE_GUIDANCE_TEXT_BUDGET,
            ));
            guidance.push('\n');
        }
    }
    next.prompt.push(LlmMessage::user_text(guidance));
    next
}

fn push_tool_result_message(out: &mut LlmPrompt, text: String) {
    if let Some(LlmMessage::Assistant { content, .. }) = out.last_mut() {
        content.push(AssistantContentPart::text(text));
    } else {
        out.push(LlmMessage::assistant_text(text));
    }
}

fn user_text(parts: &[UserContentPart]) -> String {
    let mut out = String::new();
    for part in parts {
        match part {
            UserContentPart::Text(text) => push_section(&mut out, &text.text),
            UserContentPart::File(file) => {
                push_section(&mut out, &format!("[file: {}]", file.media_type));
            }
        }
    }
    out
}

fn assistant_text(parts: &[AssistantContentPart]) -> String {
    let mut out = String::new();
    for part in parts {
        match part {
            AssistantContentPart::Text(text) => push_section(&mut out, &text.text),
            AssistantContentPart::Reasoning(reasoning) => push_section(&mut out, &reasoning.text),
            AssistantContentPart::ToolCall(call) => {
                push_section(
                    &mut out,
                    &format!("[called tool: {}({})]", call.tool_name, call.input),
                );
            }
            AssistantContentPart::ToolResult(result) => {
                push_section(
                    &mut out,
                    &format!(
                        "[tool result: {} {}]",
                        result.tool_name,
                        tool_result_content_text(&result.output)
                    ),
                );
            }
            AssistantContentPart::File(_)
            | AssistantContentPart::ReasoningFile(_)
            | AssistantContentPart::Custom(_)
            | AssistantContentPart::Source(_)
            | AssistantContentPart::ToolApprovalRequest(_) => {}
        }
    }
    out
}

fn tool_text(parts: &[ToolContentPart]) -> String {
    let mut out = String::new();
    for part in parts {
        match part {
            ToolContentPart::ToolResult(result) => push_section(
                &mut out,
                &format!(
                    "[tool result: {} {}]",
                    result.tool_name,
                    tool_result_content_text(&result.output)
                ),
            ),
            ToolContentPart::ToolApprovalResponse(response) => push_section(
                &mut out,
                &format!(
                    "[tool approval: {} approved={}]",
                    response.approval_id, response.approved
                ),
            ),
        }
    }
    out
}

fn tool_result_content_text(content: &ToolResultContent) -> String {
    let text = match content {
        ToolResultContent::Text { value, .. } | ToolResultContent::ErrorText { value, .. } => {
            value.clone()
        }
        ToolResultContent::Json { value, .. } | ToolResultContent::ErrorJson { value, .. } => {
            value.to_string()
        }
        ToolResultContent::ExecutionDenied { reason, .. } => {
            reason.as_deref().unwrap_or("execution denied").to_string()
        }
        ToolResultContent::Content { value, .. } => value
            .iter()
            .filter_map(|part| match part {
                ToolResultContentPart::Text { text, .. } => Some(text.as_str()),
                ToolResultContentPart::FileData { .. }
                | ToolResultContentPart::FileUrl { .. }
                | ToolResultContentPart::FileReference { .. }
                | ToolResultContentPart::Custom { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    };
    truncate_head_tail(&text, TOOL_RESULT_TEXT_BUDGET)
}

/// Floor on an advisor's input budget. A model declaring a window smaller than
/// its own output reserve would otherwise compute a negative budget and get an
/// empty prompt; give it something rather than nothing.
const MIN_ADVISOR_INPUT_TOKENS: i64 = 1_000;

/// Input-token budget for one advisor: its own context window minus its own
/// output reserve, both read from that model's `ModelInfo`.
///
/// `None` when the registry cannot resolve the model — the caller then sends
/// the prompt untrimmed rather than inventing a bound, which keeps an unknown
/// model behaving exactly as it did before per-advisor fitting existed.
fn advisor_input_budget(
    model_runtimes: &Arc<ModelRuntimeRegistry>,
    selection: &ProviderModelSelection,
) -> Option<i64> {
    let info = model_runtimes
        .primary_model_info_for_source(ModelRuntimeSource::Explicit(selection.clone()))
        .ok()
        .flatten()?;
    let window = i64::from(info.context_window.get());
    let reserve = i64::from(info.max_output_tokens.get());
    Some((window - reserve).max(MIN_ADVISOR_INPUT_TOKENS))
}

/// Estimated tokens for one advisor-prompt message. The advisor prompt is
/// text-only by construction (`reference_prompt` flattens every part), so the
/// text estimator is exact enough for a windowing decision.
fn estimate_advisor_message_tokens(message: &LlmMessage) -> i64 {
    let text = match message {
        LlmMessage::System { content, .. } | LlmMessage::Developer { content, .. } => {
            user_text(content)
        }
        LlmMessage::User { content, .. } => user_text(content),
        LlmMessage::Assistant { content, .. } => assistant_text(content),
        LlmMessage::Tool { content, .. } => tool_text(content),
    };
    coco_messages::estimate_text_tokens(&text)
}

/// Fit the shared advisor prompt into one advisor's own context window by
/// dropping the oldest middle turns.
///
/// A preset routinely mixes a small cheap advisor with a large one; a single
/// shared budget either overflows the small model (400 / silent provider-side
/// truncation, so its guidance comes back mangled) or wastes the large one's
/// room. The leading system message and the trailing message are never
/// dropped — without them the advisor has no instructions and no question.
///
/// If the prompt still exceeds the budget once only those two remain, it is
/// sent as-is with a warning rather than clipped: a silently truncated question
/// yields confidently wrong advice, which is worse than a visible failure the
/// aggregator already renders as `[failed: …]`.
fn fit_prompt_to_advisor(
    mut prompt: LlmPrompt,
    budget: Option<i64>,
    provider: &str,
    model_id: &str,
) -> LlmPrompt {
    let Some(budget) = budget else {
        return prompt;
    };
    let mut total: i64 = prompt.iter().map(estimate_advisor_message_tokens).sum();
    if total <= budget {
        return prompt;
    }
    let before = total;
    let dropped_from = prompt.len();
    while total > budget && prompt.len() > 2 {
        total -= estimate_advisor_message_tokens(&prompt[1]);
        prompt.remove(1);
    }
    if total > budget {
        tracing::warn!(
            target: "coco::moa",
            provider, model_id, budget, estimated = total,
            "advisor prompt exceeds its context window even after dropping history; sending as-is"
        );
    } else {
        tracing::debug!(
            target: "coco::moa",
            provider, model_id, budget,
            estimated_before = before, estimated_after = total,
            dropped_messages = dropped_from - prompt.len(),
            "advisor prompt fitted to its own context window"
        );
    }
    prompt
}

fn truncate_head_tail(text: &str, budget: usize) -> String {
    if text.len() <= budget {
        return text.to_string();
    }
    let half = budget / 2;
    let head = coco_utils_string::take_bytes_at_char_boundary(text, half);
    let tail = coco_utils_string::take_last_bytes_at_char_boundary(text, budget - half);
    format!("{head}\n...[truncated]...\n{tail}")
}

fn push_section(out: &mut String, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(text.trim());
}

#[cfg(test)]
#[path = "moa.test.rs"]
mod tests;
