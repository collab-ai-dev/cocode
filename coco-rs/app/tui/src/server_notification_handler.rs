//! Server notification handler — processes protocol events from the agent loop.
//!
//! Architecture (post WS-2 refactor): this module receives `CoreEvent` from
//! the agent loop and dispatches directly to three exhaustive handlers:
//!
//! - [`protocol::handle`] — all `ServerNotification` variants (exhaustive)
//! - [`stream::handle`] — all `AgentStreamEvent` variants (exhaustive)
//! - [`tui_only::handle`] — TUI-exclusive `TuiOnlyEvent` variants
//!
//! The old `TuiNotification` bridge type has been deleted. See
//! `event-system-design.md` §1.7-1.8 and plan file WS-2 for rationale:
//! - 75% of variants were trivial pass-throughs with no real adaptation
//! - Scaling to 57 variants would create a 1:1 copy, tripling maintenance
//! - The TUI is not classical TEA; `TuiNotification` was a private
//!   intermediate for one of two orthogonal dispatch paths
//!
//! # Per-layer submodules
//!
//! Each `CoreEvent` layer owns its own file under
//! `server_notification_handler/`; the exhaustive-match arms stay scannable
//! because per-variant logic is extracted into named private functions (and,
//! for the turn-lifecycle family, into `protocol/turn.rs`).
//!
//! - `protocol.rs` — the flat `ServerNotification` match (~62 arms); its
//!   `protocol/turn.rs` child owns the `TurnEnded` outcome family,
//!   auto-restore, and the session-boundary cleanup
//! - `stream.rs` — `AgentStreamEvent` arms
//! - `tui_only.rs` — `TuiOnlyEvent` arms + diff-stats, rewind, elicitation
//!   helpers

use coco_types::CoreEvent;
use tokio::sync::mpsc::Sender;

use crate::command::UserCommand;
use crate::state::AppState;

mod projection;
mod protocol;
mod stream;
mod tui_only;

/// Handle a `CoreEvent` from the agent loop.
///
/// Dispatches to the per-layer handler. Each layer matches exhaustively —
/// adding a new variant in `coco-types` fails compilation in the matching
/// submodule until a TUI behavior is chosen.
///
/// `command_tx` lets handlers fire a follow-up [`UserCommand`] without
/// the older two-step `pending_*` field dance. Handlers call
/// `command_tx.try_send(...)` directly when an event must round-trip
/// back to the engine (e.g. auto-restore, TUI-originated system
/// messages). See `engine-tui-unified-transcript-plan.md` §6.4.
///
/// Returns `true` if any state changed and a redraw is needed.
pub fn handle_core_event(
    state: &mut AppState,
    event: CoreEvent,
    command_tx: &Sender<UserCommand>,
) -> bool {
    // Top-level breadcrumb. Per-arm handlers in `protocol.rs` / `tui_only.rs`
    // emit richer logs for the variants that matter; this one is the
    // "did we even receive the event" hook for forensics. Stream deltas
    // (TextDelta / ThinkingDelta) fire per-chunk so they get `trace!`;
    // everything else is debug.
    match &event {
        CoreEvent::Stream(coco_types::AgentStreamEvent::TextDelta { .. })
        | CoreEvent::Stream(coco_types::AgentStreamEvent::ThinkingDelta { .. }) => {
            tracing::trace!(target: "coco_tui::core_event", layer = "stream", "stream delta");
        }
        CoreEvent::Protocol(notif) => tracing::debug!(
            target: "coco_tui::core_event",
            layer = "protocol",
            variant = core_event_variant(notif),
            "CoreEvent fold",
        ),
        CoreEvent::Stream(s) => tracing::debug!(
            target: "coco_tui::core_event",
            layer = "stream",
            variant = stream_event_variant(s),
            "CoreEvent fold",
        ),
        CoreEvent::Tui(t) => tracing::debug!(
            target: "coco_tui::core_event",
            layer = "tui",
            variant = tui_event_variant(t),
            "CoreEvent fold",
        ),
    }
    match event {
        CoreEvent::Protocol(notif) => protocol::handle(state, notif, command_tx),
        CoreEvent::Stream(stream_evt) => stream::handle(state, stream_evt),
        CoreEvent::Tui(tui_evt) => tui_only::handle(state, tui_evt, command_tx),
    }
}

/// Cheap variant-name extraction for logging. Avoids paying the `Debug`
/// price for the full payload on the per-event hot path.
fn core_event_variant(notif: &coco_types::ServerNotification) -> &'static str {
    use coco_types::ServerNotification as N;
    match notif {
        N::SessionStarted(_) => "SessionStarted",
        N::SessionResult(_) => "SessionResult",
        N::SessionEnded(_) => "SessionEnded",
        N::SessionUsageUpdated(_) => "SessionUsageUpdated",
        N::TurnStarted(_) => "TurnStarted",
        N::TurnEnded(_) => "TurnEnded",
        N::ItemStarted { .. } => "ItemStarted",
        N::ItemUpdated { .. } => "ItemUpdated",
        N::ItemCompleted { .. } => "ItemCompleted",
        N::AgentMessageDelta(_) => "AgentMessageDelta",
        N::ReasoningDelta(_) => "ReasoningDelta",
        N::MessageAppended { .. } => "MessageAppended",
        N::MessageTruncated { .. } => "MessageTruncated",
        N::SessionResetForResume { .. } => "SessionResetForResume",
        N::ReasoningMetadataAttached(_) => "ReasoningMetadataAttached",
        N::HistoryReplaced { .. } => "HistoryReplaced",
        N::ToolUseSummary(_) => "ToolUseSummary",
        N::ToolProgress(_) => "ToolProgress",
        N::SummarizeCompleted(_) => "SummarizeCompleted",
        N::SummarizeFailed { .. } => "SummarizeFailed",
        N::StreamStallDetected { .. } => "StreamStallDetected",
        N::StreamWatchdogWarning { .. } => "StreamWatchdogWarning",
        N::StreamRequestEnd { .. } => "StreamRequestEnd",
        N::SessionStateChanged { .. } => "SessionStateChanged",
        N::LocalCommandOutput(_) => "LocalCommandOutput",
        N::FilesPersisted(_) => "FilesPersisted",
        N::ElicitationComplete(_) => "ElicitationComplete",
        N::PluginsChanged { .. } => "PluginsChanged",
        N::WorktreeExited(_) => "WorktreeExited",
        _ => "Other",
    }
}

fn stream_event_variant(s: &coco_types::AgentStreamEvent) -> &'static str {
    use coco_types::AgentStreamEvent as E;
    match s {
        E::TextDelta { .. } => "TextDelta",
        E::ThinkingDelta { .. } => "ThinkingDelta",
        E::ToolUseQueued { .. } => "ToolUseQueued",
        E::ToolUseStarted { .. } => "ToolUseStarted",
        E::ToolUseCompleted { .. } => "ToolUseCompleted",
        E::McpToolCallBegin { .. } => "McpToolCallBegin",
        E::McpToolCallEnd { .. } => "McpToolCallEnd",
    }
}

fn tui_event_variant(t: &coco_types::TuiOnlyEvent) -> &'static str {
    use coco_types::TuiOnlyEvent as E;
    match t {
        E::ApprovalRequired { .. } => "ApprovalRequired",
        E::RewindRowMetadataReady { .. } => "RewindRowMetadataReady",
        E::RewindRestorePreviewReady { .. } => "RewindRestorePreviewReady",
        E::RewindCompleted { .. } => "RewindCompleted",
        E::QuestionAsked { .. } => "QuestionAsked",
        E::ElicitationRequested { .. } => "ElicitationRequested",
        E::SandboxApprovalRequired { .. } => "SandboxApprovalRequired",
        E::PermissionExplanationReady { .. } => "PermissionExplanationReady",
        E::PluginDataReady { .. } => "PluginDataReady",
        E::OutputStylesReady { .. } => "OutputStylesReady",
        E::AvailableCommandsRefreshed { .. } => "AvailableCommandsRefreshed",
        E::QueuedCommandEditReady { .. } => "QueuedCommandEditReady",
        E::QueuedCommandsEditReady { .. } => "QueuedCommandsEditReady",
        E::QueuedCommandEditUnavailable { .. } => "QueuedCommandEditUnavailable",
        E::OpenSessionBrowser { .. } => "OpenSessionBrowser",
        E::CompactionCircuitBreakerOpen { .. } => "CompactionCircuitBreakerOpen",
        E::MicroCompactionApplied { .. } => "MicroCompactionApplied",
        E::SessionMemoryCompactApplied { .. } => "SessionMemoryCompactApplied",
        E::SpeculativeRolledBack { .. } => "SpeculativeRolledBack",
        E::SessionMemoryExtractionStarted => "SessionMemoryExtractionStarted",
        E::SessionMemoryExtractionCompleted { .. } => "SessionMemoryExtractionCompleted",
        E::SessionMemoryExtractionFailed { .. } => "SessionMemoryExtractionFailed",
        E::CronJobDisabled { .. } => "CronJobDisabled",
        E::CronJobsMissed { .. } => "CronJobsMissed",
        E::ToolCallStreamStart { .. } => "ToolCallStreamStart",
        E::ToolCallDelta { .. } => "ToolCallDelta",
        E::ToolProgress { .. } => "ToolProgress",
        E::ToolInterruptibilityChanged { .. } => "ToolInterruptibilityChanged",
        E::ToolExecutionAborted { .. } => "ToolExecutionAborted",
        E::SlashCommandResult { .. } => "SlashCommandResult",
        E::OpenContextUsage { .. } => "OpenContextUsage",
        E::OpenRewindPicker => "OpenRewindPicker",
        E::OpenMemoryDialog { .. } => "OpenMemoryDialog",
        E::CopyCommandRequested { .. } => "CopyCommandRequested",
        E::MemoryFileOpened { .. } => "MemoryFileOpened",
        E::MemoryFileOpenFailed { .. } => "MemoryFileOpenFailed",
        E::PlanFileOpened { .. } => "PlanFileOpened",
        E::PlanFileOpenFailed { .. } => "PlanFileOpenFailed",
        E::ExternalEditorPrepare { .. } => "ExternalEditorPrepare",
        E::PromptEditorCompleted { .. } => "PromptEditorCompleted",
        E::PromptEditorFailed { .. } => "PromptEditorFailed",
        E::BashCommandCompleted { .. } => "BashCommandCompleted",
        E::OpenModelPicker => "OpenModelPicker",
        E::OpenSettings => "OpenSettings",
        E::OpenThemePicker => "OpenThemePicker",
        E::OpenSkillsDialog { .. } => "OpenSkillsDialog",
        E::OpenPluginDialog { .. } => "OpenPluginDialog",
        E::OpenAgentsDialog { .. } => "OpenAgentsDialog",
        E::OpenPermissionsEditor { .. } => "OpenPermissionsEditor",
        E::OpenAddDirectory => "OpenAddDirectory",
        E::OpenExport => "OpenExport",
        E::SkillOverridesSaved { .. } => "SkillOverridesSaved",
        E::SlashCommandStatus { .. } => "SlashCommandStatus",
    }
}

/// Thin wrapper around [`handle_core_event`] that constructs a
/// dummy `Sender<UserCommand>` internally. Intended for tests that
/// don't need to assert on dispatched commands. Tests that DO need
/// to observe commands should call [`handle_core_event`] directly
/// with their own channel.
///
/// The receiver lives for the duration of the call, so any
/// `try_send` inside the handler succeeds; the message is then
/// dropped when the receiver goes out of scope — equivalent to
/// /dev/null for dispatched follow-up commands.
///
/// Gated behind the `testing` feature (the crate's own `tests/` get it via
/// the self dev-dependency that enables `testing`); it must not ship in
/// release builds.
#[cfg(any(test, feature = "testing"))]
pub fn handle_event_for_test(state: &mut AppState, event: CoreEvent) -> bool {
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    handle_core_event(state, event, &tx)
}

#[cfg(test)]
#[path = "server_notification_handler.test.rs"]
mod tests;
