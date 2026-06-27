//! Custom system_prompt is honored: when callers supply an explicit
//! `system_prompt` via `QueryEngineConfig`, the engine must thread it
//! through to the API call instead of composing its own. The model
//! should follow the supplied directive on the very first turn.
//!
//! Engine wiring: `QueryEngineConfig.system_prompt: Option<String>` —
//! `Some` means "use this verbatim, skip the composed prompt"; `None`
//! means "let `coco-context` build one". The CLI's `--system-prompt`
//! flag and the SDK's `initialize.system_prompt` both flow into this
//! field. Regression target: a refactor that accidentally drops or
//! overwrites the override would let composed prompt content win.
//!
//! Distinct from `one_shot`: that test exercises the default composed
//! prompt path. This test pins the override path.

use anyhow::Result;

use crate::cli::events;
use crate::cli::harness::SessionConfig;
use crate::cli::harness::run_session;

pub async fn run(provider: &str, model: &str) -> Result<()> {
    // Marker is unique enough that no realistic free-form reply would
    // contain it by accident.
    const MARKER: &str = "ZX42-OVERRIDE-OK";

    let cfg = SessionConfig {
        max_turns: Some(2),
        max_output_tokens: 512,
        system_prompt: Some(format!(
            "You are a test fixture. ALWAYS end every reply with the exact \
             literal token {MARKER} on its own line. Do not omit it under \
             any circumstance."
        )),
        ..SessionConfig::default()
    };

    let outcome = run_session(provider, model, cfg, "Reply with: ready").await?;

    let response = outcome.result.response_text.clone();
    assert!(
        response.contains(MARKER),
        "{provider}/{model}: system_prompt override not honored — marker \
         {MARKER:?} missing from response. response={response:?} events={}",
        events::summarize(&outcome.events),
    );
    Ok(())
}
