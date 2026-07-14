//! Headless (print-mode) surface composition.
//!
//! Owns the CLI-side one-shot policy: `--resume`/`--continue`/`--fork-session`
//! resolution, `RunChatOptions` assembly, stdout/stderr presentation of the
//! outcome, and shutdown-drain result handling. The reusable orchestration
//! (`run_chat_with_options`, config/model/prompt resolution) stays in
//! `coco_agent_host::headless`; this module only composes it into the `coco -p`
//! process surface.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;

use coco_agent_host::resume_resolver;

use crate::Cli;
use crate::shutdown_drain_result;

/// Run a single-turn print mode (--print / piped stdout).
pub(crate) async fn run_chat(
    cli: &Cli,
    prompt: Option<&str>,
    cwd: PathBuf,
    process_runtime: Arc<coco_app_runtime::ProcessRuntime>,
) -> Result<()> {
    let agent_host_options = cli.agent_host_options();
    // Resolve `--resume` / `--continue` / `--fork-session` once at
    // the boot edge so headless and TUI share identical semantics.
    // `None` means no resume flag was set; fall through to a fresh
    // session.
    let runtime_paths = coco_agent_host::paths::runtime_paths();
    let plan = resume_resolver::resolve(&agent_host_options, runtime_paths.memory_base(), &cwd)?;
    if let Some(p) = &plan {
        eprintln!(
            "{} session {} ({} prior message(s))",
            if p.is_fork { "Forked" } else { "Resumed" },
            p.source_session_id,
            p.prior_messages.len(),
        );
    }
    let opts = match plan {
        Some(p) => coco_agent_host::headless::RunChatOptions {
            cwd: Some(cwd.clone()),
            prior_messages: p
                .prior_messages
                .into_iter()
                .map(std::sync::Arc::new)
                .collect(),
            session_id_override: Some(p.session_id.clone()),
            resume_target: Some(coco_types::SessionTarget {
                session_id: p.session_id,
            }),
            stored_mode: p.conversation.mode,
            process_runtime: Some(process_runtime.clone()),
            ..Default::default()
        },
        None => coco_agent_host::headless::RunChatOptions {
            cwd: Some(cwd.clone()),
            process_runtime: Some(process_runtime.clone()),
            ..Default::default()
        },
    };
    let outcome =
        coco_agent_host::headless::run_chat_with_options(&agent_host_options, prompt, opts).await?;
    if let Some(msg) = &outcome.permission_notification {
        tracing::warn!(target: "coco_agent_host::headless", notice = %msg, "headless permission notice");
        eprintln!("warning: {msg}");
    }
    let mode = outcome
        .provider_api
        .map_or("mock", coco_types::ProviderApi::as_str);
    tracing::info!(
        target: "coco_agent_host::headless",
        provider_mode = mode,
        model_id = %outcome.model_id,
        turns = outcome.turns,
        tokens_in = outcome.total_usage.input_tokens.total,
        tokens_out = outcome.total_usage.output_tokens.total,
        "headless chat complete"
    );
    eprintln!("coco-rs ({mode} mode) — model: {}\n", outcome.model_id);
    println!("{}", outcome.response_text);
    eprintln!(
        "\n─── {} turn(s) | {} in / {} out tokens ───",
        outcome.turns,
        outcome.total_usage.input_tokens.total,
        outcome.total_usage.output_tokens.total
    );
    shutdown_drain_result("headless AppServer", &outcome.app_server_shutdown)?;
    shutdown_drain_result("headless Event Hub", &outcome.event_hub_shutdown)
}
