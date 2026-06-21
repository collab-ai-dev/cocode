//! Shared `/rename` helpers used by both the interactive TUI runner
//! and the SDK / headless runner.
//!
//! The runner-specific pieces (teammate guard, transcript echo,
//! `emit_slash_text`) stay in each runner; the LLM call and the
//! dual-write persistence both live here so the SDK and TUI paths
//! can't drift apart.
//!
//! ## Side-effects intentionally NOT performed here (G6 / G7)
//!
//! - **Bridge title sync**: claude.ai CCR-backend-only — not a
//!   coco-rs target. See `coco-rs/bridge/CLAUDE.md`
//!   "Deliberately Not Ported".
//! - **`AppState.standaloneAgentContext.name` propagation**: used
//!   to drive the prompt-bar banner. coco-rs has no live
//!   `coco_state::AppState` instance and no banner widget reading
//!   `standalone_agent_context`. Populating dead state is forbidden
//!   by CLAUDE.md ("Don't design for hypothetical future
//!   requirements"). When a TUI banner lands, wire the reader and
//!   add the producer call here in the **same** PR.

use std::sync::Arc;

use coco_types::Capability;
use coco_types::ModelRole;
use coco_types::SideQueryToolDef;
use tracing::warn;

use crate::session_runtime::SessionRuntime;

const RENAME_GENERATE_NAME_QUERY_SOURCE: &str = "rename_generate_name";

/// User-facing failure message returned to the runner when an
/// auto-rename attempt cannot produce a name.
///
/// - `NoConversation` — no messages after the compact boundary.
/// - `LlmFailed` — provider error / timeout / parse mismatch;
///   surfaces "try again or pass a name".
///
/// All variants render to a single short user line. Detailed
/// diagnostics go to `tracing::warn` inside [`auto_generate_session_name`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoRenameError {
    NoConversation,
    LlmFailed,
}

impl AutoRenameError {
    pub fn user_message(&self) -> &'static str {
        match self {
            Self::NoConversation => {
                "Need conversation context first. Send a message and try again, \
                 or pass a name: /rename <name>"
            }
            Self::LlmFailed => "Couldn't generate name. Try again, or pass one: /rename <name>",
        }
    }
}

/// Build a kebab-case session name via the `ModelRole::Fast` resolver.
///
/// Snapshots `messages_after_compact_boundary` off the runtime's
/// history, builds the conversation text, then tries native structured
/// output before falling back to a forced `generate_session_name` tool
/// call.
pub async fn auto_generate_session_name(
    runtime: &Arc<SessionRuntime>,
) -> Result<String, AutoRenameError> {
    let conversation_text = snapshot_conversation_text(runtime).await;
    generate_session_name_from_text(runtime.side_query(), conversation_text).await
}

/// Generate a session name from already-extracted text. Used by bare
/// `/rename` with conversation text and by post-plan auto-name with the
/// accepted plan head slice.
pub async fn generate_session_name_from_text(
    handle: coco_tool_runtime::SideQueryHandle,
    text: String,
) -> Result<String, AutoRenameError> {
    if text.trim().is_empty() {
        return Err(AutoRenameError::NoConversation);
    }

    let (system, user) = coco_session::title_generator::build_session_name_prompt(&text);
    let schema = coco_session::title_generator::session_name_schema();

    if handle.supports_capability(Some(ModelRole::Fast), Capability::StructuredOutput) {
        let request = coco_tool_runtime::SideQueryRequest::with_json_schema(
            &system,
            &user,
            schema.clone(),
            RENAME_GENERATE_NAME_QUERY_SOURCE,
        )
        .with_schema_name(coco_session::title_generator::SESSION_NAME_TOOL_NAME)
        .with_schema_description("A short kebab-case session name.")
        .with_model_role(ModelRole::Fast)
        .with_skip_system_prefix(true);

        match handle.query(request).await {
            Ok(resp) => {
                if let Some(name) = resp
                    .text
                    .as_deref()
                    .and_then(coco_session::title_generator::parse_session_name_response)
                {
                    return Ok(name);
                }
                warn!(
                    "rename auto-gen: structured output was malformed; falling back to forced tool"
                );
            }
            Err(err) => {
                warn!(error = %err, "rename auto-gen: structured output query failed; falling back to forced tool");
            }
        }
    }

    let tool = SideQueryToolDef {
        name: coco_session::title_generator::SESSION_NAME_TOOL_NAME.to_string(),
        description: "Return a short kebab-case session name.".to_string(),
        input_schema: schema,
    };
    let request = coco_tool_runtime::SideQueryRequest::with_forced_tool(
        &system,
        &user,
        tool,
        RENAME_GENERATE_NAME_QUERY_SOURCE,
    )
    .with_model_role(ModelRole::Fast)
    .with_skip_system_prefix(true);

    let resp = handle.query(request).await.map_err(|err| {
        warn!(error = %err, "rename auto-gen: forced-tool query failed");
        AutoRenameError::LlmFailed
    })?;
    resp.tool_uses
        .iter()
        .find(|tool_use| {
            tool_use.name == coco_session::title_generator::SESSION_NAME_TOOL_NAME
                && !tool_use.invalid
        })
        .and_then(|tool_use| {
            coco_session::title_generator::parse_session_name_tool_input(&tool_use.input)
        })
        .ok_or_else(|| {
            warn!("rename auto-gen: forced-tool response was malformed");
            AutoRenameError::LlmFailed
        })
}

/// Persist a resolved rename. Two side effects, in order:
///
/// 1. `SessionManager::set_title` appends both `CustomTitle` and
///    `AgentName` metadata entries to the JSONL transcript.
/// 2. `SessionRegistry::update_session_name` live-patches the
///    `<config_home>/sessions/<pid>.json` file so `coco ps` reflects
///    the new name. Best-effort — silent when the session isn't
///    registered (subagent context, FS-constrained startup).
///
/// `name` MUST be non-empty; the caller is responsible for resolving
/// any auto-generation upstream. Errors surface as
/// `anyhow::Error` so callers can match on
/// `SessionError::TranscriptNotFound` for a clearer message.
pub async fn persist_rename(
    runtime: &Arc<SessionRuntime>,
    name: String,
) -> Result<(), anyhow::Error> {
    let session_id = runtime.current_session_id().await;
    let manager = runtime.session_manager.clone();
    let name_for_set = name.clone();
    let session_id_for_set = session_id.clone();
    tokio::task::spawn_blocking(move || manager.set_title(&session_id_for_set, &name_for_set))
        .await
        .map_err(anyhow::Error::from)
        .and_then(|inner| inner.map_err(anyhow::Error::from))?;
    runtime.update_session_registry_name(&name);
    Ok(())
}

/// Snapshot the conversation text used for auto-name generation.
/// Walks the post-compact-boundary slice of the in-memory history,
/// concatenating text from User / Assistant messages.
/// Non-text content (tool calls, attachments, etc.) is skipped.
async fn snapshot_conversation_text(runtime: &Arc<SessionRuntime>) -> String {
    let history = runtime.history.lock().await;
    coco_session::title_generator::extract_conversation_text(history.as_slice())
}

#[cfg(test)]
#[path = "session_rename.test.rs"]
mod tests;
