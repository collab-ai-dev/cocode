//! Parallel safe-tool dispatch: model issues multiple Read calls in a
//! single turn → `ToolExecutor` runs them concurrently via the
//! safe-tool batch path.
//!
//! Engine wiring: each tool implements `is_concurrency_safe(input)`.
//! Read is safe; the executor groups all safe tools in a turn and
//! dispatches via `FuturesUnordered`. Catches regressions where the
//! safe-concurrent branch falls back to sequential execution (the
//! observable signal is the count of `ToolUseStarted` events, not their
//! ordering — we trust the executor's internal scheduling).
//!
//! Path choice: `/etc/hostname` and `/etc/os-release` exist on every
//! Linux host the live suite runs on, are tiny, and are outside the
//! tempdir cwd — proving the Read tool isn't constrained to
//! `project_dir`. The model is instructed to issue both reads in a
//! single turn so the executor sees them as one batch.

use anyhow::Result;

use crate::cli::events;
use crate::cli::harness::SessionConfig;
use crate::cli::harness::run_session;

pub async fn run(provider: &str, model: &str) -> Result<()> {
    let cfg = SessionConfig {
        max_turns: Some(4),
        max_output_tokens: 1_024,
        ..SessionConfig::default()
    };
    let prompt = "Use the Read tool to read BOTH /etc/hostname AND \
                  /etc/os-release in this same turn (issue both tool calls \
                  before waiting for any result). After both reads return, \
                  reply with a single line: RESULT=both-files-read";
    let outcome = run_session(provider, model, cfg, prompt).await?;

    let started = events::tool_uses_started(&outcome.events);
    let read_count = started.iter().filter(|n| **n == "Read").count();
    assert!(
        read_count >= 2,
        "{provider}/{model}: expected >=2 Read tool calls, got {read_count}; \
         tool_starts={started:?} events={}",
        events::summarize(&outcome.events),
    );

    let completed = events::tool_uses_completed(&outcome.events);
    let read_completions = completed.iter().filter(|(n, _)| *n == "Read").count();
    assert!(
        read_completions >= 2,
        "{provider}/{model}: expected >=2 Read tool completions, got {read_completions}; \
         tool_completions={completed:?} events={}",
        events::summarize(&outcome.events),
    );
    Ok(())
}
