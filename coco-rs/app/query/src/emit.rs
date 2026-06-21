//! Typed emission helpers for the agent loop.
//!
//! Producers of `CoreEvent` inside `coco-query` (and downstream crates)
//! send events via an `Option<mpsc::Sender<CoreEvent>>` — `Some(tx)` when
//! a consumer is attached (TUI, SDK dispatcher, bridge), `None` for
//! headless / test paths that don't care about events.
//!
//! Without these helpers every emission site writes:
//!
//! ```ignore
//! if let Some(tx) = &event_tx {
//!     let _ = tx.send(CoreEvent::Protocol(ServerNotification::TurnStarted(p))).await;
//! }
//! ```
//!
//! Problems with that pattern:
//! - `let _ =` swallows send errors silently (no way to know the channel closed).
//! - No layering discipline — callers construct `CoreEvent::X(Y(p))` inline.
//! - No observability — no tracing or metrics at emission points.
//! - Four call sites for every emission × ~15 events × ~3 consumers is noisy.
//!
//! These helpers centralize the pattern:
//! - [`emit`] / [`emit_protocol`] / [`emit_stream`] / [`emit_tui`] wrap the
//!   `Option<Sender>` check, emit a `tracing::trace!` event with the layer +
//!   method for filtering, record a counter via [`coco_otel::metrics`],
//!   and return `bool` so callers can react to a closed channel.
//! - [`emit_protocol_owned`] variant for producers that already hold a
//!   non-Option sender (hook forwarder, task manager).
//!
//! # Returned bool
//!
//! - `true`  = delivered, **or** no sender attached (headless success).
//! - `false` = sender was closed / receiver dropped.
//!
//! Callers that treat the return value as meaningful must not ignore it; the
//! `#[must_use]` attribute enforces this at the call site. Most sites in the
//! agent loop bind the result to `_delivered` or check it explicitly to
//! short-circuit further work.
//!
//! # Observability
//!
//! Each typed helper records into two OTel counters (via the global handle
//! in [`coco_otel::metrics`]) — zero cost when no exporter is attached:
//!
//! - `coco.events.emitted_total{layer, method}` — successful deliveries.
//! - `coco.events.channel_closed_total{layer, method}` — sender dropped,
//!   consumer side gone. One per failed send, not per attempt.
//!
//! Headless mode (`tx = None`) records no metrics — there is no real
//! delivery to measure.
//!
//! # Ordering
//!
//! These helpers do NOT change ordering semantics — they're thin wrappers
//! over `Sender::send`. The per-task FIFO ordering contract documented in
//! `coco_types::CoreEvent` still applies.

use coco_types::AgentStreamEvent;
use coco_types::CoreEvent;
use coco_types::ServerNotification;
use coco_types::TuiOnlyEvent;
use tokio::sync::mpsc::Sender;

const LAYER_PROTOCOL: &str = "protocol";
const LAYER_STREAM: &str = "stream";
const LAYER_TUI: &str = "tui";

/// Emit any `CoreEvent`. Returns `true` if delivered or if `tx` is `None`
/// (headless); `false` if the receiver has been dropped.
///
/// Prefer the typed helpers (`emit_protocol`, `emit_stream`, `emit_tui`)
/// when the layer is known at the call site — they add a structured
/// `tracing::trace!` with the method/variant for log filtering.
#[must_use = "ignoring the delivery result hides a closed event channel; bind to `_delivered` if intentional"]
pub async fn emit(tx: &Option<Sender<CoreEvent>>, event: CoreEvent) -> bool {
    let Some(sender) = tx else {
        return true;
    };
    sender.send(event).await.is_ok()
}

/// Emit a protocol-layer notification. Adds a `tracing::trace!` event with
/// `layer="protocol"` and `method=<wire method>` so log queries can audit
/// emission patterns without parsing the full event payload.
#[must_use = "ignoring the delivery result hides a closed event channel; bind to `_delivered` if intentional"]
pub async fn emit_protocol(tx: &Option<Sender<CoreEvent>>, notif: ServerNotification) -> bool {
    let method = notif.method().as_str();
    tracing::trace!(layer = LAYER_PROTOCOL, method, "emit");
    let headless = tx.is_none();
    let delivered = emit(tx, CoreEvent::Protocol(notif)).await;
    if !headless {
        record_emit_metric(LAYER_PROTOCOL, method, delivered);
    }
    delivered
}

/// Emit a stream-layer event (raw agent-loop stream deltas).
///
/// Stream events are high-frequency on the hot path (one `TextDelta` per
/// token). We log at `trace` level to keep production noise low; bump to
/// `debug` when diagnosing a stream-accumulator mismatch.
#[must_use = "ignoring the delivery result hides a closed event channel; bind to `_delivered` if intentional"]
pub async fn emit_stream(tx: &Option<Sender<CoreEvent>>, evt: AgentStreamEvent) -> bool {
    let kind = stream_kind(&evt);
    tracing::trace!(layer = LAYER_STREAM, kind, "emit");
    let headless = tx.is_none();
    let delivered = emit(tx, CoreEvent::Stream(evt)).await;
    if !headless {
        record_emit_metric(LAYER_STREAM, kind, delivered);
    }
    delivered
}

/// Emit a TUI-only event (overlays, toasts, UI-specific metadata).
/// SDK and bridge consumers drop these; the trace event captures where they
/// were produced.
#[must_use = "ignoring the delivery result hides a closed event channel; bind to `_delivered` if intentional"]
pub async fn emit_tui(tx: &Option<Sender<CoreEvent>>, evt: TuiOnlyEvent) -> bool {
    let kind = tui_kind(&evt);
    tracing::trace!(layer = LAYER_TUI, kind, "emit");
    let headless = tx.is_none();
    let delivered = emit(tx, CoreEvent::Tui(evt)).await;
    if !headless {
        record_emit_metric(LAYER_TUI, kind, delivered);
    }
    delivered
}

/// Emit a protocol notification on an owned (non-Option) sender.
/// Used by the hook forwarder child task and other producers that have
/// already unwrapped the optional sender at task-spawn time.
#[must_use = "ignoring the delivery result hides a closed event channel; bind to `_delivered` if intentional"]
pub async fn emit_protocol_owned(tx: &Sender<CoreEvent>, notif: ServerNotification) -> bool {
    let method = notif.method().as_str();
    tracing::trace!(layer = LAYER_PROTOCOL, method, "emit");
    let delivered = tx.send(CoreEvent::Protocol(notif)).await.is_ok();
    record_emit_metric(LAYER_PROTOCOL, method, delivered);
    delivered
}

/// Record a counter increment for an emission.
///
/// Routes to `coco.events.emitted_total` on success and
/// `coco.events.channel_closed_total` on failure, tagged with the layer
/// and the method (for protocol) or kind (for stream/tui). Zero-cost when
/// no OTel exporter is attached — the underlying `record_counter` short-
/// circuits on `None` without allocating or locking.
fn record_emit_metric(layer: &'static str, method_or_kind: &str, delivered: bool) {
    let name = if delivered {
        "coco.events.emitted_total"
    } else {
        "coco.events.channel_closed_total"
    };
    coco_otel::metrics::record_counter(name, 1, &[("layer", layer), ("method", method_or_kind)]);
}

/// Tag for `AgentStreamEvent` variants — avoids allocating the full
/// `Debug` representation just for the trace log.
fn stream_kind(evt: &AgentStreamEvent) -> &'static str {
    match evt {
        AgentStreamEvent::TextDelta { .. } => "text_delta",
        AgentStreamEvent::ThinkingDelta { .. } => "thinking_delta",
        AgentStreamEvent::ToolUseQueued { .. } => "tool_use_queued",
        AgentStreamEvent::ToolUseStarted { .. } => "tool_use_started",
        AgentStreamEvent::ToolUseCompleted { .. } => "tool_use_completed",
        AgentStreamEvent::McpToolCallBegin { .. } => "mcp_tool_call_begin",
        AgentStreamEvent::McpToolCallEnd { .. } => "mcp_tool_call_end",
    }
}

/// Tag for `TuiOnlyEvent` variants.
fn tui_kind(evt: &TuiOnlyEvent) -> &'static str {
    match evt {
        TuiOnlyEvent::ApprovalRequired { .. } => "approval_required",
        TuiOnlyEvent::QuestionAsked { .. } => "question_asked",
        TuiOnlyEvent::ElicitationRequested { .. } => "elicitation_requested",
        TuiOnlyEvent::SandboxApprovalRequired { .. } => "sandbox_approval_required",
        TuiOnlyEvent::PermissionExplanationReady { .. } => "permission_explanation_ready",
        TuiOnlyEvent::PluginDataReady { .. } => "plugin_data_ready",
        TuiOnlyEvent::OutputStylesReady { .. } => "output_styles_ready",
        TuiOnlyEvent::AvailableCommandsRefreshed { .. } => "available_commands_refreshed",
        TuiOnlyEvent::QueuedCommandEditReady { .. } => "queued_command_edit_ready",
        TuiOnlyEvent::QueuedCommandsEditReady { .. } => "queued_commands_edit_ready",
        TuiOnlyEvent::QueuedCommandEditUnavailable { .. } => "queued_command_edit_unavailable",
        TuiOnlyEvent::OpenSessionBrowser { .. } => "open_session_browser",
        TuiOnlyEvent::RewindRowMetadataReady { .. } => "rewind_row_metadata_ready",
        TuiOnlyEvent::RewindRestorePreviewReady { .. } => "rewind_restore_preview_ready",
        TuiOnlyEvent::CompactionCircuitBreakerOpen { .. } => "compaction_circuit_breaker_open",
        TuiOnlyEvent::MicroCompactionApplied { .. } => "micro_compaction_applied",
        TuiOnlyEvent::SessionMemoryCompactApplied { .. } => "session_memory_compact_applied",
        TuiOnlyEvent::SpeculativeRolledBack { .. } => "speculative_rolled_back",
        TuiOnlyEvent::SessionMemoryExtractionStarted => "session_memory_extraction_started",
        TuiOnlyEvent::SessionMemoryExtractionCompleted { .. } => {
            "session_memory_extraction_completed"
        }
        TuiOnlyEvent::SessionMemoryExtractionFailed { .. } => "session_memory_extraction_failed",
        TuiOnlyEvent::CronJobDisabled { .. } => "cron_job_disabled",
        TuiOnlyEvent::CronJobsMissed { .. } => "cron_jobs_missed",
        TuiOnlyEvent::ToolCallStreamStart { .. } => "tool_call_stream_start",
        TuiOnlyEvent::ToolCallDelta { .. } => "tool_call_delta",
        TuiOnlyEvent::ToolProgress { .. } => "tool_progress",
        TuiOnlyEvent::ToolInterruptibilityChanged { .. } => "tool_interruptibility_changed",
        TuiOnlyEvent::ToolExecutionAborted { .. } => "tool_execution_aborted",
        TuiOnlyEvent::RewindCompleted { .. } => "rewind_completed",
        TuiOnlyEvent::SlashCommandResult { .. } => "slash_command_result",
        TuiOnlyEvent::SlashCommandStatus { .. } => "slash_command_status",
        TuiOnlyEvent::OpenContextUsage { .. } => "open_context_usage",
        TuiOnlyEvent::OpenRewindPicker => "open_rewind_picker",
        TuiOnlyEvent::OpenMemoryDialog { .. } => "open_memory_dialog",
        TuiOnlyEvent::CopyCommandRequested { .. } => "copy_command_requested",
        TuiOnlyEvent::MemoryFileOpened { .. } => "memory_file_opened",
        TuiOnlyEvent::MemoryFileOpenFailed { .. } => "memory_file_open_failed",
        TuiOnlyEvent::PlanFileOpened { .. } => "plan_file_opened",
        TuiOnlyEvent::PlanFileOpenFailed { .. } => "plan_file_open_failed",
        TuiOnlyEvent::ExternalEditorPrepare { .. } => "external_editor_prepare",
        TuiOnlyEvent::PromptEditorCompleted { .. } => "prompt_editor_completed",
        TuiOnlyEvent::PromptEditorFailed { .. } => "prompt_editor_failed",
        TuiOnlyEvent::BashCommandCompleted { .. } => "bash_command_completed",
        TuiOnlyEvent::OpenModelPicker => "open_model_picker",
        TuiOnlyEvent::OpenSettings => "open_settings",
        TuiOnlyEvent::OpenThemePicker => "open_theme_picker",
        TuiOnlyEvent::OpenSkillsDialog { .. } => "open_skills_dialog",
        TuiOnlyEvent::OpenPluginDialog { .. } => "open_plugin_dialog",
        TuiOnlyEvent::OpenAgentsDialog { .. } => "open_agents_dialog",
        TuiOnlyEvent::OpenPermissionsEditor { .. } => "open_permissions_editor",
        TuiOnlyEvent::OpenAddDirectory => "open_add_directory",
        TuiOnlyEvent::OpenExport => "open_export",
        TuiOnlyEvent::SkillOverridesSaved { .. } => "skill_overrides_saved",
    }
}

#[cfg(test)]
#[path = "emit.test.rs"]
mod tests;
