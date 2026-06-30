use std::sync::Arc;
use std::sync::Mutex;

use coco_messages::AssistantContent;
use coco_messages::Message;
use coco_messages::create_api_error_message;
use coco_messages::create_assistant_message;
use coco_query::forked_agent::CanUseToolCallContext;
use coco_query::forked_agent::CanUseToolDecision;
use coco_query::forked_agent::ForkDispatcher;
use coco_query::forked_agent::ForkDispatcherRef;
use coco_query::forked_agent::ForkedAgentOptions;
use coco_query::forked_agent::ForkedAgentResult;
use coco_types::CacheSafeParams;
use coco_types::ForkLabel;
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::SIDE_QUESTION_SYSTEM_REMINDER;
use super::extract_side_question_answer;
use super::run_side_question_fork;

fn assistant(text: &str) -> Arc<Message> {
    Arc::new(create_assistant_message(
        vec![AssistantContent::text(text)],
        "test-model",
        Default::default(),
    ))
}

fn assistant_tool_call(tool_name: &str) -> Arc<Message> {
    Arc::new(create_assistant_message(
        vec![AssistantContent::ToolCall(
            coco_llm_types::ToolCallPart::new("toolu_1", tool_name, json!({})),
        )],
        "test-model",
        Default::default(),
    ))
}

fn empty_cache() -> CacheSafeParams {
    CacheSafeParams {
        rendered_system_prompt: "system".into(),
        model_id: "test-model".into(),
        provider: "test-provider".into(),
        active_shell_tool: coco_types::ActiveShellTool::Disabled,
        prompt_cache: None,
        fork_context_messages: Vec::new(),
    }
}

#[test]
fn extract_joins_text_across_per_block_messages() {
    // The provider yields one assistant message per content block, so the
    // answer must concatenate text across ALL of them — not just the last
    // (the old single-message walk dropped earlier blocks).
    let msgs = vec![assistant("part one"), assistant("part two")];
    assert_eq!(extract_side_question_answer(&msgs), "part one\n\npart two");
}

#[test]
fn extract_skips_empty_text_messages() {
    // A leading thinking-only message extracts to empty text and must be
    // skipped rather than short-circuiting to "no response".
    let msgs = vec![assistant(""), assistant("real answer")];
    assert_eq!(extract_side_question_answer(&msgs), "real answer");
}

#[test]
fn extract_no_assistant_content_returns_no_response() {
    assert_eq!(extract_side_question_answer(&[]), "(No response received.)");
}

#[test]
fn extract_reports_attempted_tool_name() {
    let msgs = vec![assistant_tool_call("Read")];
    assert_eq!(
        extract_side_question_answer(&msgs),
        "(The model tried to call Read instead of answering directly. Try rephrasing or ask in the main conversation.)"
    );
}

#[test]
fn extract_reports_api_error() {
    let msgs = vec![Arc::new(create_api_error_message(
        "rate limited",
        Some(429),
    ))];
    assert_eq!(
        extract_side_question_answer(&msgs),
        "(API error: rate limited)"
    );
}

#[test]
fn system_reminder_matches_btw_toolless_contract() {
    assert!(SIDE_QUESTION_SYSTEM_REMINDER.contains("This is a side question from the user"));
    assert!(SIDE_QUESTION_SYSTEM_REMINDER.contains("The main agent is NOT interrupted"));
    assert!(SIDE_QUESTION_SYSTEM_REMINDER.contains("You have NO tools available"));
    assert!(SIDE_QUESTION_SYSTEM_REMINDER.contains("there will be no follow-up turns"));
}

#[derive(Default)]
struct RecordingDispatcher {
    options: Mutex<Option<ForkedAgentOptions>>,
    prompt: Mutex<Option<String>>,
}

#[async_trait::async_trait]
impl ForkDispatcher for RecordingDispatcher {
    async fn dispatch(
        &self,
        _cache: &CacheSafeParams,
        options: &ForkedAgentOptions,
        prompt: &str,
        _system_prompt_override: Option<String>,
    ) -> Result<ForkedAgentResult, coco_error::BoxedError> {
        *self.options.lock().unwrap() = Some(options.clone());
        *self.prompt.lock().unwrap() = Some(prompt.to_string());
        Ok(ForkedAgentResult {
            messages: vec![assistant("direct answer")],
            total_usage: Default::default(),
        })
    }
}

#[tokio::test]
async fn run_fork_sets_side_question_tool_policy() {
    let recorder = Arc::new(RecordingDispatcher::default());
    let dispatcher: ForkDispatcherRef = recorder.clone();
    let answer = run_side_question_fork(&empty_cache(), &dispatcher, "what changed?").await;

    assert_eq!(answer, "direct answer");
    let options = recorder.options.lock().unwrap().clone().unwrap();
    assert_eq!(options.fork_label, ForkLabel::SideQuestion);
    assert_eq!(options.query_source, "side_question");
    assert_eq!(options.max_turns, Some(1));
    assert!(options.skip_cache_write);
    assert!(options.can_use_tool.is_some());
    assert!(
        options.require_can_use_tool,
        "side questions must not allow hooks to bypass the no-tools policy"
    );

    let ctx = CanUseToolCallContext {
        tool_use_id: "toolu_1".into(),
        cwd: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/")),
        abort: coco_tool_runtime::TurnAbortSignal::from_token(CancellationToken::new()),
        require_can_use_tool: options.require_can_use_tool,
        messages: Arc::new(Vec::new()),
    };
    let decision = options
        .can_use_tool
        .unwrap()
        .check("Read", &json!({}), &ctx)
        .await;
    match decision {
        CanUseToolDecision::Deny { message, .. } => {
            assert!(message.contains("Side questions cannot use tools"));
        }
        other => panic!("expected side question tool denial, got {other:?}"),
    }

    let prompt = recorder.prompt.lock().unwrap().clone().unwrap();
    assert!(prompt.starts_with(SIDE_QUESTION_SYSTEM_REMINDER));
    assert!(prompt.ends_with("what changed?"));
}
