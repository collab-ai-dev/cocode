//! Skill-listing reminder gating.
//!
//! The reminder should be model-visible only when the current filtered
//! loaded tool set includes `Skill`. Otherwise it teaches the model to
//! call a tool that is not actually available on this turn.

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
use coco_llm_types::AssistantContentPart;
use coco_llm_types::FinishReason;
use coco_llm_types::LlmMessage;
use coco_llm_types::StopReason;
use coco_llm_types::TextPart;
use coco_llm_types::ToolCallPart;
use coco_llm_types::Usage;
use coco_system_reminder::InvokedSkillEntry;
use coco_system_reminder::ReminderSources;
use coco_system_reminder::SkillsSource;
use coco_tool_runtime::ToolRegistry;
use coco_types::AttachmentKind;
use coco_types::PermissionMode;
use coco_types::ToolFilter;
use coco_types::ToolName;
use tokio_util::sync::CancellationToken;

use crate::QueryEngine;
use crate::QueryEngineConfig;

const LISTING_MARKER: &str = "SKILL-LISTING-MARKER";

#[derive(Debug)]
struct CapturingTextModel {
    captured_prompts: Arc<Mutex<Vec<Vec<LlmMessage>>>>,
}

#[derive(Debug)]
struct InvalidStructuredOutputModel;

#[test]
fn workflow_keyword_matcher_ignores_code_and_paths() {
    assert!(super::contains_unmasked_workflow_keyword(
        "please ultracode this"
    ));
    assert!(super::contains_unmasked_workflow_keyword(
        "please ULTRACODE this"
    ));
    assert!(!super::contains_unmasked_workflow_keyword(
        "please inspect `ultracode`"
    ));
    assert!(!super::contains_unmasked_workflow_keyword(
        "open docs/ultracode.md"
    ));
    assert!(!super::contains_unmasked_workflow_keyword("/ultracode"));
    assert!(!super::contains_unmasked_workflow_keyword(
        "please inspect 'ultracode'"
    ));
    assert!(!super::contains_unmasked_workflow_keyword(
        "please inspect \"ultracode\""
    ));
    assert!(!super::contains_unmasked_workflow_keyword(
        "<tag ultracode=\"true\">"
    ));
    assert!(!super::contains_unmasked_workflow_keyword(
        "please (ultracode) this"
    ));
    assert!(!super::contains_unmasked_workflow_keyword(
        "please [ultracode] this"
    ));
    assert!(!super::contains_unmasked_workflow_keyword(
        "please {ultracode} this"
    ));
    assert!(!super::contains_unmasked_workflow_keyword(
        "run --ultracode"
    ));
    // `word.ultracode` (keyword after a dot, at the end) is prose in TS — only
    // `ultracode.word` member access is suppressed.
    assert!(super::contains_unmasked_workflow_keyword(
        "config.ultracode"
    ));
    assert!(!super::contains_unmasked_workflow_keyword("ultracode.foo"));
    assert!(!super::contains_unmasked_workflow_keyword(
        "search ultracode?mode=1"
    ));
    // Sentence-final keyword fires (the trailing '.' is not member access).
    assert!(super::contains_unmasked_workflow_keyword(
        "let's ultracode."
    ));
    assert!(super::contains_unmasked_workflow_keyword(
        "ultracode, please"
    ));
    // Backslash-glued forms do not fire (path/flag rejection on both sides).
    assert!(!super::contains_unmasked_workflow_keyword(
        r"run \ultracode"
    ));
    assert!(!super::contains_unmasked_workflow_keyword(r"ultracode\foo"));
    // `a < b` math must not open a phantom `<` span that swallows the keyword.
    assert!(super::contains_unmasked_workflow_keyword(
        "if a < b ultracode now"
    ));
}

#[test]
fn structured_output_enforcement_nudges_until_success() {
    let mut history = coco_messages::MessageHistory::new();
    history.push(coco_messages::create_user_message("answer as json"));

    assert!(super::should_fire_structured_output_enforcement(
        &history, true
    ));

    history.push(super::structured_output_enforcement_message());
    assert!(
        !super::should_fire_structured_output_enforcement(&history, true),
        "sentinel nudge should dedupe within the current user-turn window"
    );

    history.push(coco_messages::Message::Attachment(
        coco_messages::AttachmentMessage::silent_structured_output(
            coco_messages::StructuredOutputPayload {
                data: serde_json::json!({"answer": 42}),
            },
        ),
    ));
    assert!(
        !super::should_fire_structured_output_enforcement(&history, true),
        "schema-valid StructuredOutput attachment satisfies the contract"
    );
}

#[test]
fn structured_output_enforcement_resets_for_next_user_turn() {
    let mut history = coco_messages::MessageHistory::new();
    history.push(coco_messages::create_user_message("first"));
    history.push(super::structured_output_enforcement_message());
    assert!(!super::should_fire_structured_output_enforcement(
        &history, true
    ));

    history.push(coco_messages::create_user_message("second"));
    assert!(
        super::should_fire_structured_output_enforcement(&history, true),
        "a prior sentinel before the latest user turn must not suppress \
         enforcement for the new request"
    );
    assert!(!super::should_fire_structured_output_enforcement(
        &history, false
    ));
}

fn structured_output_tools() -> Arc<ToolRegistry> {
    let registry = ToolRegistry::new();
    coco_tools::register_structured_output_tool(
        &registry,
        serde_json::json!({
            "type": "object",
            "properties": {
                "answer": { "type": "string" }
            },
            "required": ["answer"]
        }),
    )
    .expect("valid structured output schema");
    Arc::new(registry)
}

#[tokio::test]
async fn structured_output_no_tool_turns_stop_at_retry_cap() {
    let max_retries = 3;
    let captured = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(CapturingTextModel {
        captured_prompts: captured.clone(),
    });
    let client = crate::test_support::model_runtime_registry(model);
    let config = QueryEngineConfig {
        model_id: "skill-listing-mock".into(),
        permission_mode: PermissionMode::Default,
        max_turns: Some(max_retries as i32 + 1),
        requires_structured_output: true,
        max_structured_output_retries: max_retries,
        ..Default::default()
    };
    let engine = QueryEngine::new(
        config,
        client,
        structured_output_tools(),
        CancellationToken::new(),
        None,
    );
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<coco_types::CoreEvent>(64);

    let result = engine
        .run_with_events("answer as json", event_tx, coco_types::TurnId::generate())
        .await
        .expect("engine run");

    assert_eq!(
        result.stop_reason.as_deref(),
        Some("error_max_structured_output_retries"),
        "text-only replies should count toward the StructuredOutput retry cap"
    );
    let mut events = Vec::new();
    while let Ok(event) = event_rx.try_recv() {
        events.push(event);
    }
    let turn_ended = events.iter().find_map(|event| match event {
        coco_types::CoreEvent::Protocol(coco_types::ServerNotification::TurnEnded(params)) => {
            Some(params)
        }
        _ => None,
    });
    let turn_ended = turn_ended.expect("retry cap should emit TurnEnded");
    match &turn_ended.outcome {
        coco_types::TurnOutcome::Failed(outcome) => {
            assert_eq!(outcome.error.code, coco_types::ErrorCode::Provider);
            assert!(
                outcome.error.message.contains("3 attempts"),
                "failed outcome should include the configured cap"
            );
        }
        other => panic!("expected failed TurnEnded outcome, got {other:?}"),
    }
    let prompts = captured.lock().expect("captured prompts lock").clone();
    assert_eq!(prompts.len(), max_retries as usize);
    let second_prompt = prompt_text(&prompts[1]);
    assert_eq!(
        second_prompt.matches("[structured-output-enforce]").count(),
        1,
        "the terminal no-tool branch must not append duplicate enforcement \
         nudges; pre-turn reminder injection owns the sentinel dedupe"
    );
}

#[tokio::test]
async fn structured_output_tool_failure_cap_emits_failed_turn() {
    let client =
        crate::test_support::model_runtime_registry(Arc::new(InvalidStructuredOutputModel));
    let config = QueryEngineConfig {
        model_id: "structured-output-invalid-mock".into(),
        permission_mode: PermissionMode::Default,
        max_turns: Some(2),
        requires_structured_output: true,
        max_structured_output_retries: 1,
        ..Default::default()
    };
    let engine = QueryEngine::new(
        config,
        client,
        structured_output_tools(),
        CancellationToken::new(),
        None,
    );
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<coco_types::CoreEvent>(64);

    let result = engine
        .run_with_events("answer as json", event_tx, coco_types::TurnId::generate())
        .await
        .expect("engine run");

    assert_eq!(
        result.stop_reason.as_deref(),
        Some("error_max_structured_output_retries")
    );
    let mut outcomes = Vec::new();
    while let Ok(event) = event_rx.try_recv() {
        if let coco_types::CoreEvent::Protocol(coco_types::ServerNotification::TurnEnded(params)) =
            event
        {
            outcomes.push(params.outcome);
        }
    }
    assert_eq!(
        outcomes.len(),
        1,
        "retry cap should emit one terminal event"
    );
    assert!(
        matches!(outcomes[0], coco_types::TurnOutcome::Failed(_)),
        "retry cap should be a failed turn, got {:?}",
        outcomes[0]
    );
}

#[async_trait]
impl LanguageModel for CapturingTextModel {
    fn provider(&self) -> &str {
        "mock"
    }

    fn model_id(&self) -> &str {
        "skill-listing-mock"
    }

    async fn do_generate(
        &self,
        options: &LanguageModelCallOptions,
        _abort_signal: Option<CancellationToken>,
    ) -> Result<LanguageModelGenerateResult, AISdkError> {
        self.captured_prompts
            .lock()
            .expect("captured prompts lock")
            .push(options.prompt.clone());
        Ok(LanguageModelGenerateResult {
            content: vec![AssistantContentPart::Text(TextPart {
                text: "done".into(),
                provider_metadata: None,
            })],
            usage: Usage::new(10, 3),
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
        abort_signal: Option<CancellationToken>,
    ) -> Result<LanguageModelStreamResult, AISdkError> {
        let result = self.do_generate(options, abort_signal).await?;
        Ok(coco_inference::synthetic_stream_from_content(
            result.content,
            result.usage,
            result.finish_reason,
        ))
    }
}

#[async_trait]
impl LanguageModel for InvalidStructuredOutputModel {
    fn provider(&self) -> &str {
        "mock"
    }

    fn model_id(&self) -> &str {
        "structured-output-invalid-mock"
    }

    async fn do_generate(
        &self,
        _options: &LanguageModelCallOptions,
        _abort_signal: Option<CancellationToken>,
    ) -> Result<LanguageModelGenerateResult, AISdkError> {
        Ok(LanguageModelGenerateResult {
            content: vec![AssistantContentPart::ToolCall(ToolCallPart {
                tool_call_id: "structured-output-1".into(),
                tool_name: ToolName::StructuredOutput.as_str().into(),
                input: serde_json::json!({}),
                provider_executed: None,
                provider_metadata: None,
                invalid: false,
                invalid_reason: None,
            })],
            usage: Usage::new(10, 3),
            finish_reason: FinishReason::new(StopReason::ToolUse),
            warnings: vec![],
            provider_metadata: None,
            request: None,
            response: None,
        })
    }

    async fn do_stream(
        &self,
        options: &LanguageModelCallOptions,
        abort_signal: Option<CancellationToken>,
    ) -> Result<LanguageModelStreamResult, AISdkError> {
        let result = self.do_generate(options, abort_signal).await?;
        Ok(coco_inference::synthetic_stream_from_content(
            result.content,
            result.usage,
            result.finish_reason,
        ))
    }
}

#[derive(Debug)]
struct SpySkillsSource {
    listing_calls: AtomicUsize,
}

impl SpySkillsSource {
    fn new() -> Self {
        Self {
            listing_calls: AtomicUsize::new(0),
        }
    }

    fn listing_calls(&self) -> usize {
        self.listing_calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl SkillsSource for SpySkillsSource {
    async fn listing(
        &self,
        _agent_id: Option<&str>,
        _tiers: &coco_config::SkillOverrideTiers,
    ) -> Option<String> {
        self.listing_calls.fetch_add(1, Ordering::SeqCst);
        Some(format!("- review: {LISTING_MARKER}"))
    }

    async fn invoked(&self, _agent_id: Option<&str>) -> Vec<InvokedSkillEntry> {
        Vec::new()
    }

    async fn activate_skills_for_paths(
        &self,
        _file_paths: &[std::path::PathBuf],
        _cwd: &std::path::Path,
    ) -> Vec<String> {
        Vec::new()
    }
}

fn skill_tools() -> Arc<ToolRegistry> {
    let registry = ToolRegistry::new();
    registry.register(Arc::new(coco_tools::SkillTool));
    Arc::new(registry)
}

async fn run_case(config: QueryEngineConfig) -> (Vec<Vec<LlmMessage>>, usize, bool) {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(CapturingTextModel {
        captured_prompts: captured.clone(),
    });
    let source = Arc::new(SpySkillsSource::new());
    let client = crate::test_support::model_runtime_registry(model);
    let engine = QueryEngine::new(
        config,
        client,
        skill_tools(),
        CancellationToken::new(),
        None,
    )
    .with_reminder_sources(ReminderSources {
        skills: Some(source.clone()),
        ..Default::default()
    });

    let result = engine.run("hello").await.expect("engine run");
    let has_skill_listing = result.final_messages.iter().any(|message| {
        matches!(
            message.as_ref(),
            coco_messages::Message::Attachment(att) if att.kind == AttachmentKind::SkillListing
        )
    });
    let prompts = captured.lock().expect("captured prompts lock").clone();
    (prompts, source.listing_calls(), has_skill_listing)
}

fn prompt_text(prompt: &[LlmMessage]) -> String {
    prompt
        .iter()
        .map(extract_all_text)
        .collect::<Vec<_>>()
        .join("\n")
}

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
        LlmMessage::User { content, .. }
        | LlmMessage::System { content, .. }
        | LlmMessage::Developer { content, .. } => {
            for part in content {
                if let UserContentPart::Text(t) = part {
                    push(&t.text);
                }
            }
        }
        LlmMessage::Assistant { content, .. } => {
            for part in content {
                if let AssistantContentPart::Text(t) = part {
                    push(&t.text);
                }
            }
        }
        LlmMessage::Tool { content, .. } => {
            for part in content {
                if let ToolContentPart::ToolResult(result) = part {
                    match &result.output {
                        ToolResultContent::Text { value, .. } => push(value),
                        ToolResultContent::Content { value, .. } => {
                            for part in value {
                                if let coco_llm_types::ToolResultContentPart::Text {
                                    text, ..
                                } = part
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
    }
    out
}

#[tokio::test]
async fn skill_listing_default_mode_with_skill_tool_injects() {
    let config = QueryEngineConfig {
        model_id: "skill-listing-mock".into(),
        permission_mode: PermissionMode::Default,
        max_turns: Some(1),
        ..Default::default()
    };

    let (prompts, listing_calls, has_skill_listing) = run_case(config).await;

    assert_eq!(listing_calls, 1);
    assert!(has_skill_listing);
    assert!(
        prompt_text(&prompts[0]).contains(LISTING_MARKER),
        "skill listing should reach the model when Skill is loaded"
    );
}

#[tokio::test]
async fn skill_listing_plan_mode_keeps_skill_tool_and_injects() {
    // Plan mode no longer strips the Skill tool from the schema
    // (layer-3 removal), so Skill stays in the loaded set and the
    // skill-listing reminder is injected exactly as in Default mode.
    // Skill execution is gated at call time by the permission layer, not
    // by hiding the tool — so teaching the model about skills is correct.
    let config = QueryEngineConfig {
        model_id: "skill-listing-mock".into(),
        permission_mode: PermissionMode::Plan,
        max_turns: Some(1),
        ..Default::default()
    };

    let (prompts, listing_calls, has_skill_listing) = run_case(config).await;

    assert_eq!(listing_calls, 1);
    assert!(has_skill_listing);
    assert!(
        prompt_text(&prompts[0]).contains(LISTING_MARKER),
        "skill listing should reach the model in plan mode (Skill is no longer stripped)"
    );
}

#[tokio::test]
async fn skill_listing_tool_filter_excluding_skill_suppresses() {
    let config = QueryEngineConfig {
        model_id: "skill-listing-mock".into(),
        permission_mode: PermissionMode::Default,
        tool_filter: ToolFilter::new(Vec::new(), vec![ToolName::Skill.as_str().into()]),
        max_turns: Some(1),
        ..Default::default()
    };

    let (prompts, listing_calls, has_skill_listing) = run_case(config).await;

    assert_eq!(listing_calls, 0);
    assert!(!has_skill_listing);
    assert!(
        !prompt_text(&prompts[0]).contains(LISTING_MARKER),
        "skill listing should not reach the model when Skill is filtered out"
    );
}

/// `active_agent_mentions` strips the `agent-` prefix from the unquoted
/// form and drops mentions that don't
/// resolve to an active agent type, so the reminder never tells the model to
/// invoke an agent `AgentTool` would reject as an unknown `subagent_type`.
#[test]
fn test_active_agent_mentions_strips_prefix_and_filters_unknown() {
    use coco_context::user_input::process_user_input;

    let input = process_user_input("Try @agent-Explore then @agent-bogus then @\"Plan (agent)\"");
    let active = vec!["Explore".to_string(), "Plan".to_string()];

    let entries = super::active_agent_mentions(&input.mentions, &active);
    let types: Vec<&str> = entries.iter().map(|e| e.agent_type.as_str()).collect();

    // `agent-Explore` → `Explore` (prefix stripped, known → kept);
    // `agent-bogus` → dropped (not in catalog);
    // `"Plan (agent)"` → `Plan` (suffix-stripped form, known → kept).
    assert_eq!(types, vec!["Explore", "Plan"]);
}

/// With no active agents, every `@agent-…` mention is dropped (fail-closed).
#[test]
fn test_active_agent_mentions_empty_catalog_drops_all() {
    use coco_context::user_input::process_user_input;

    let input = process_user_input("Use @agent-Explore please");
    let entries = super::active_agent_mentions(&input.mentions, &[]);
    assert!(entries.is_empty());
}

/// Regression guard (the headline fix): the `@agent-…` mention filter must read
/// the WIRED `agent_catalog` — the same catalog `AgentTool::execute` validates
/// `subagent_type` against — and NOT `session_bootstrap.agents`, which is
/// `None` in every production path (TUI/SDK/headless). Sourcing from the dead
/// field dropped every `@agent-…` mention before it reached the model,
/// defeating the reminder's entire purpose.
///
/// Drives a real `QueryEngine` turn end-to-end with a built-in catalog (which
/// includes `Explore`) and asserts the agent-mention reminder reaches the
/// model. The unit tests above exercise the helper in isolation; only an
/// engine-level test catches the wrong *source* being wired in.
#[tokio::test]
async fn agent_mentions_reminder_sources_from_wired_catalog() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(CapturingTextModel {
        captured_prompts: captured.clone(),
    });
    let client = crate::test_support::model_runtime_registry(model);

    // Built-in catalog with Explore/Plan enabled — the production source.
    let mut store = coco_subagent::AgentDefinitionStore::new(
        coco_subagent::BuiltinAgentCatalog::all_enabled(),
        coco_subagent::AgentSearchPaths::empty(),
    );
    store.load();
    let catalog = store.snapshot();

    let config = QueryEngineConfig {
        model_id: "skill-listing-mock".into(),
        permission_mode: PermissionMode::Default,
        max_turns: Some(1),
        ..Default::default()
    };
    let engine = QueryEngine::new(
        config,
        client,
        skill_tools(),
        CancellationToken::new(),
        None,
    )
    .with_agent_catalog(catalog);

    engine
        .run("Please use @agent-Explore on this")
        .await
        .expect("engine run");

    let prompts = captured.lock().expect("captured prompts lock").clone();
    assert!(
        prompt_text(&prompts[0]).contains("invoke the agent \"Explore\""),
        "the @agent-Explore mention reminder must reach the model when Explore \
         is active in the wired catalog (regression: it was sourced from the \
         dead session_bootstrap.agents and silently dropped in production)"
    );
}
