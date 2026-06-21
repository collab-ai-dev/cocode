//! `AgentStreamEvent::ToolUseCompleted` must carry a non-empty `name`
//! field for every completion, so downstream consumers
//! (`StreamAccumulator`, TUI, SDK clients) can reconstruct display
//! state by tool name without maintaining their own `call_id → name`
//! map.
//!
//! Engine wiring (event.rs:122): `ToolUseCompleted { call_id, name,
//! output, is_error }`. The comment on that variant explicitly notes:
//! "name is carried here so StreamAccumulator and TUI consumers can
//! reconstruct display state without maintaining their own call_id →
//! name map." A regression that drops the name (e.g. an executor
//! refactor that emits `name: ""`) silently breaks downstream renders;
//! this test pins it.
//!
//! Test design: kick a tool-chain prompt that runs ≥1 Bash and ≥1
//! Read, drain the event stream, and assert every
//! `ToolUseCompleted` has a populated `name`.

use anyhow::Result;
use coco_types::AgentStreamEvent;
use coco_types::CoreEvent;

use crate::cli::events;
use crate::cli::harness::SessionConfig;
use crate::cli::harness::run_session;

pub async fn run(provider: &str, model: &str) -> Result<()> {
    let cfg = SessionConfig {
        max_turns: Some(6),
        max_output_tokens: 1_024,
        ..SessionConfig::default()
    };
    let prompt = "Step 1: Use the Bash tool to run `echo hello-from-bash`. \
                  Step 2: Use the Read tool to read /etc/hostname. \
                  After both, reply: RESULT=both-tools-ran";
    let outcome = run_session(provider, model, cfg, prompt).await?;

    let completions: Vec<&AgentStreamEvent> = outcome
        .events
        .iter()
        .filter_map(|e| match e {
            CoreEvent::Stream(s @ AgentStreamEvent::ToolUseCompleted { .. }) => Some(s),
            _ => None,
        })
        .collect();

    assert!(
        !completions.is_empty(),
        "{provider}/{model}: no ToolUseCompleted events at all; events={}",
        events::summarize(&outcome.events),
    );

    for evt in &completions {
        if let AgentStreamEvent::ToolUseCompleted {
            call_id,
            name,
            is_error,
            ..
        } = evt
        {
            assert!(
                !name.trim().is_empty(),
                "{provider}/{model}: ToolUseCompleted has empty name (call_id={call_id}, \
                 is_error={is_error}); a regression dropped the call_id→name carry. \
                 events={}",
                events::summarize(&outcome.events),
            );
        }
    }
    Ok(())
}
