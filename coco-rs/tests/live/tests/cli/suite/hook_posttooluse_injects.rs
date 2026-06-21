//! PostToolUse hook injects additional context: a registered
//! `PostToolUse` hook with a `Prompt` handler emits text that the
//! engine threads back into the conversation. The next turn's model
//! response must reflect that injected text.
//!
//! Hook protocol: `HookHandler::Prompt { prompt , model: None }` returns the
//! verbatim string as `HookExecutionResult::PromptText`. The
//! orchestration layer attaches it as a silent additional-context
//! message before the next API call.
//!
//! Engine wiring: PostToolUse fires after `Tool::execute()` returns
//! and before the next runtime query call. The injected text is
//! visible to the model as part of its next-turn context, so a
//! prompt asking it to echo a marker that ONLY exists in the hook
//! injection proves the wiring end-to-end.

use std::sync::Arc;

use anyhow::Result;
use coco_hooks::HookDefinition;
use coco_hooks::HookHandler;
use coco_hooks::HookRegistry;
use coco_types::HookEventType;

use crate::cli::events;
use crate::cli::harness::SessionConfig;
use crate::cli::harness::run_session;

pub async fn run(provider: &str, model: &str) -> Result<()> {
    const MARKER: &str = "AURORA-9988";

    let registry = HookRegistry::new();
    registry.register(HookDefinition {
        event: HookEventType::PostToolUse,
        matcher: Some("Bash".to_string()),
        handler: HookHandler::Prompt {
            prompt: format!(
                "Hook injected this side-channel context: the post-tool \
                 marker is {MARKER}. Whenever the user asks for the \
                 marker, recite this exact string verbatim."
            ),
            model: None,
            timeout_ms: None,
        },
        priority: 0,
        scope: Default::default(),
        if_condition: None,
        once: false,
        is_async: false,
        async_rewake: false,
        status_message: None,
    });

    let cfg = SessionConfig {
        max_turns: Some(5),
        max_output_tokens: 1_024,
        hooks: Some(Arc::new(registry)),
        ..SessionConfig::default()
    };

    // First turn runs Bash → fires the PostToolUse hook → the marker
    // is added to the conversation. Second prompt asks the model to
    // recite it. We pack both into one user message because
    // `run_session` is single-prompt.
    let prompt = "Step 1: Use the Bash tool to run `echo started`. \
                  Step 2: After it returns, the system will inject a \
                  side-channel marker. Tell me what the marker is — \
                  reply with the literal marker and nothing else.";
    let outcome = run_session(provider, model, cfg, prompt).await?;

    let started = events::tool_uses_started(&outcome.events);
    assert!(
        started.contains(&"Bash"),
        "{provider}/{model}: expected Bash to run so PostToolUse fires; \
         tool_starts={started:?}",
    );
    assert!(
        outcome.result.response_text.contains(MARKER),
        "{provider}/{model}: PostToolUse-injected marker {MARKER:?} not in \
         response. response={:?} events={}",
        outcome.result.response_text,
        events::summarize(&outcome.events),
    );
    Ok(())
}
