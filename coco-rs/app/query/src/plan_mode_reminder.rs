//! Per-turn plan-mode **side effects** (no reminder emission).
//!
//! Since Phase D.3 the reminder subsystem lives in `coco-system-reminder`:
//! plan/auto/todo/task/compaction/date-change reminders are emitted by the
//! [`SystemReminderOrchestrator`] from the shared context built in
//! `coco-query::engine`. This file covers only the per-turn side effects
//! the orchestrator can't do:
//!
//! - **Mode reconciliation** — detect an unannounced plan/auto mode
//!   transition (Shift+Tab, SDK `setPermissionMode`, etc.) and set the
//!   `has_exited_plan_mode` / `needs_plan_mode_exit_attachment` /
//!   `needs_auto_mode_exit_attachment` flags on app_state so the
//!   orchestrator emits the matching banner this turn.
//! - **Teammate approval polling** — drain the mailbox for the leader's
//!   outstanding plan approval and inject a synthetic approval /
//!   rejection reminder into history (plus update app_state).
//! - **Leader pending approvals** — scan the leader's inbox for unread
//!   `plan_approval_request` messages and inject a summary reminder
//!   (plus surface each to the TUI via `PlanApprovalRequested`).
//!
//! Plan-mode reminder cadence is **not** here — it is history-derived inside
//! `coco-system-reminder`'s plan/auto generators.
//!
//! See the swarm mailbox protocol for `plan_approval_request` /
//! `_response` message formats.

use coco_messages::AttachmentMessage;
use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_messages::MessageHistory;
use coco_messages::wrapping::wrap_in_system_reminder;
use coco_types::PermissionMode;
use coco_types::ToolAppState;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Stateful per-turn side-effect driver for plan-mode / auto-mode transitions
/// and the swarm plan-approval handshake.
///
/// Held for a single `run_session_loop` invocation. The durable reminder
/// cadence state lives on `app_state`, not on this struct, so cadence
/// survives across runs.
pub struct PlanModeReminder {
    /// Fallback permission mode used only when `app_state.permission_mode`
    /// is `None` (typically isolated unit tests that don't seed app_state).
    fallback_permission_mode: PermissionMode,
    /// Session identifier (reserved for future side-effects that need to
    /// resolve per-session paths).
    #[allow(dead_code)]
    session_id: Option<String>,
    /// Optional active agent ID (subagents don't drive the team mailbox).
    #[allow(dead_code)]
    agent_id: Option<String>,
    /// Plans directory (reserved for future plan-file-touching side effects).
    #[allow(dead_code)]
    plans_dir: Option<PathBuf>,
    /// Shared typed app_state. When `Some`, writes flow through here so
    /// subsequent turns (and the orchestrator building its context from
    /// this snapshot) observe the latest flags.
    app_state: Option<Arc<RwLock<ToolAppState>>>,
    /// Mailbox handle — enables teammate approval-response polling and
    /// leader pending-approvals attachment. `None` in non-swarm sessions.
    mailbox: Option<coco_tool_runtime::MailboxHandleRef>,
    /// Agent identity for mailbox scoping. Required for polling; if
    /// `None`, approval poll is skipped.
    agent_name: Option<String>,
    /// Team name for mailbox scoping.
    team_name: Option<String>,
    /// Set when this engine runs AS a teammate whose role requires
    /// leader approval. Enables approval-response polling.
    is_teammate_awaiting: bool,
    /// Optional protocol-event sink for surfacing plan-approval requests
    /// to the leader's TUI as `ServerNotification::PlanApprovalRequested`.
    event_tx: Option<tokio::sync::mpsc::Sender<coco_types::CoreEvent>>,
}

impl PlanModeReminder {
    pub fn new(
        permission_mode: PermissionMode,
        session_id: Option<String>,
        agent_id: Option<String>,
        plans_dir: Option<PathBuf>,
        app_state: Option<Arc<RwLock<ToolAppState>>>,
    ) -> Self {
        Self {
            fallback_permission_mode: permission_mode,
            session_id,
            agent_id,
            plans_dir,
            app_state,
            mailbox: None,
            agent_name: None,
            team_name: None,
            is_teammate_awaiting: false,
            event_tx: None,
        }
    }

    /// Resolve the current live permission mode. Reads from
    /// `app_state.permission_mode`; falls back to the constructor-time
    /// value when `app_state` is `None` or unset.
    async fn current_permission_mode(&self) -> PermissionMode {
        match self.app_state.as_ref() {
            Some(state) => state
                .read()
                .await
                .permissions
                .mode
                .unwrap_or(self.fallback_permission_mode),
            None => self.fallback_permission_mode,
        }
    }

    /// Install mailbox handle + identity for teammate approval-response
    /// polling + leader pending-approvals attachment.
    pub fn with_mailbox(
        mut self,
        mailbox: coco_tool_runtime::MailboxHandleRef,
        agent_name: String,
        team_name: String,
        is_teammate_awaiting: bool,
    ) -> Self {
        self.mailbox = Some(mailbox);
        self.agent_name = Some(agent_name);
        self.team_name = Some(team_name);
        self.is_teammate_awaiting = is_teammate_awaiting;
        self
    }

    /// Install a protocol-event sink so leader-pending-approval polling
    /// surfaces each request to the TUI as
    /// `ServerNotification::PlanApprovalRequested`.
    pub fn with_event_sink(
        mut self,
        event_tx: tokio::sync::mpsc::Sender<coco_types::CoreEvent>,
    ) -> Self {
        self.event_tx = Some(event_tx);
        self
    }

    /// Per-turn side effects the orchestrator can't run. Engine call pattern:
    ///
    /// ```ignore
    /// plan_reminder.turn_start_side_effects_only(&mut history).await;
    /// let reminders = run_turn_reminders(&orchestrator, input).await;
    /// inject_reminders(reminders, &mut history.messages);
    /// // engine clears one-shot flags for the AttachmentTypes that fired.
    /// ```
    ///
    /// Reminder emission (exit banners, plan reminder Full/Sparse, reentry,
    /// auto-mode exit, todo/task/critical/compaction/date-change) lives in
    /// `coco-system-reminder::generators`.
    pub async fn turn_start_side_effects_only(&mut self, history: &mut MessageHistory) {
        let current_mode = self.current_permission_mode().await;
        self.reconcile_mode_transition(current_mode).await;
        self.poll_teammate_approval(history).await;
        self.inject_leader_pending_approvals(history).await;
        // Plan-mode reminder cadence no longer needs a human-turn counter
        // here — the generator derives it by scanning the transcript for
        // prior `plan_mode` attachments each turn.
    }

    /// Detect and record cross-run permission-mode transitions.
    ///
    /// Plan ↔ non-Plan cycles set `has_exited_plan_mode` +
    /// `needs_plan_mode_exit_attachment`; re-entering Plan clears a stale
    /// pending exit attachment. Auto→non-Auto sets
    /// `needs_auto_mode_exit_attachment`; re-entering Auto clears a stale one.
    async fn reconcile_mode_transition(&self, current_mode: PermissionMode) {
        let Some(app_state) = self.app_state.as_ref() else {
            return;
        };
        let mut guard = app_state.write().await;
        let last_mode = guard.last_permission_mode;
        let current = current_mode;

        if let Some(prev) = last_mode {
            if prev == PermissionMode::Plan && current != PermissionMode::Plan {
                guard.has_exited_plan_mode = true;
                if !guard.needs_plan_mode_exit_attachment {
                    guard.pending_plan_mode_exit_outcome = None;
                }
                guard.needs_plan_mode_exit_attachment = true;
            } else if current == PermissionMode::Plan && prev != PermissionMode::Plan {
                guard.needs_plan_mode_exit_attachment = false;
                guard.pending_plan_mode_exit_outcome = None;
            }
            if prev == PermissionMode::Auto && current != PermissionMode::Auto {
                guard.needs_auto_mode_exit_attachment = true;
            } else if current == PermissionMode::Auto {
                guard.needs_auto_mode_exit_attachment = false;
            }
        }
        guard.last_permission_mode = Some(current);
    }

    /// Resolve the plans directory from a config_home path and optional
    /// project override. Helper so the engine can call this once when
    /// constructing the tracker.
    pub fn resolve_plans_dir(
        config_home: Option<&Path>,
        project_dir: Option<&Path>,
        plans_directory_setting: Option<&str>,
    ) -> Option<PathBuf> {
        config_home.map(|ch| {
            coco_context::resolve_plans_directory(ch, project_dir, plans_directory_setting)
        })
    }

    /// Scan the teammate's own inbox for a `plan_approval_response`
    /// matching this teammate's outstanding request. On match: inject an
    /// approval/rejection reminder, clear `awaiting_plan_approval` flags,
    /// and record the leader's override `permission_mode` so the engine
    /// picks up the mode switch on the next reconcile.
    async fn poll_teammate_approval(&self, history: &mut MessageHistory) {
        if !self.is_teammate_awaiting {
            return;
        }
        let (Some(mailbox), Some(agent), Some(team)) =
            (&self.mailbox, &self.agent_name, &self.team_name)
        else {
            return;
        };
        let Some(app_state) = &self.app_state else {
            return;
        };
        let expected_id = app_state
            .read()
            .await
            .awaiting_plan_approval_request_id
            .clone();
        let Some(expected_id) = expected_id else {
            return;
        };

        let Ok(unread) = mailbox.read_unread(agent, team).await else {
            return;
        };
        for msg in &unread {
            let Ok(coco_tool_runtime::PlanApprovalMessage::PlanApprovalResponse(resp)) =
                serde_json::from_str::<coco_tool_runtime::PlanApprovalMessage>(&msg.text)
            else {
                continue;
            };
            if resp.request_id != expected_id {
                continue;
            }

            let text = if resp.approved {
                let tail = match resp.permission_mode {
                    Some(m) => {
                        let serialized = serde_json::to_string(&m).unwrap_or_default();
                        let label = serialized.trim_matches('"');
                        format!(
                            " The team lead set your mode to `{label}`; proceed with implementation."
                        )
                    }
                    None => " Proceed with implementation.".to_string(),
                };
                format!("## Plan Approved\n\nThe team lead approved your plan.{tail}")
            } else {
                let feedback_line = resp
                    .feedback
                    .as_deref()
                    .map(|f| format!("\n\n**Feedback:** {f}"))
                    .unwrap_or_default();
                format!(
                    "## Plan Rejected\n\nThe team lead rejected your plan. Stay in plan \
                     mode and refine based on the feedback.{feedback_line}"
                )
            };
            crate::history_sync::history_push_and_emit(
                history,
                Self::raw_reminder_message(coco_types::AttachmentKind::TeammateMailbox, &text),
                &self.event_tx,
            )
            .await;

            let mut guard = app_state.write().await;
            guard.awaiting_plan_approval = false;
            guard.awaiting_plan_approval_request_id = None;
            if let Some(mode) = resp.permission_mode {
                guard.last_permission_mode = Some(mode);
            }
            drop(guard);

            let _ = mailbox.mark_read(agent, team, msg.index).await;
            return;
        }
    }

    /// Scan the leader's own inbox for unread `plan_approval_request`
    /// messages and inject an attachment summarizing what's pending.
    async fn inject_leader_pending_approvals(&self, history: &mut MessageHistory) {
        let Some(mailbox) = &self.mailbox else {
            return;
        };
        let Some(agent) = &self.agent_name else {
            return;
        };
        let Some(team) = &self.team_name else {
            return;
        };
        // Canonical leader name.
        if agent != "team-lead" {
            return;
        }

        let Ok(unread) = mailbox.read_unread(agent, team).await else {
            return;
        };
        let pending: Vec<(usize, coco_tool_runtime::PlanApprovalRequest)> = unread
            .iter()
            .filter_map(|m| {
                match serde_json::from_str::<coco_tool_runtime::PlanApprovalMessage>(&m.text)
                    .ok()?
                {
                    coco_tool_runtime::PlanApprovalMessage::PlanApprovalRequest(req) => {
                        Some((m.index, req))
                    }
                    _ => None,
                }
            })
            .collect();

        if pending.is_empty() {
            return;
        }

        let mut body = String::from(
            "## Pending Plan Approvals\n\n\
             One or more teammates have submitted plans and are waiting for your \
             review. Use the `SendMessage` tool to respond with a structured \
             `plan_approval_response` message.\n",
        );
        for (_idx, req) in &pending {
            body.push_str(&format!(
                "\n---\n**From:** `{from}`  **Request ID:** `{request_id}`  \
                 **Plan file:** `{plan_file}`\n\n{plan}\n",
                from = req.from,
                request_id = req.request_id,
                plan_file = req.plan_file_path,
                plan = req.plan_content,
            ));
        }
        body.push_str(
            "\n---\nTo approve: `SendMessage(to: \"<teammate>\", message: {{\
             type: \"plan_approval_response\", request_id: \"<id>\", approve: true}})`.\n\
             To reject with feedback: `SendMessage(to: \"<teammate>\", message: {{\
             type: \"plan_approval_response\", request_id: \"<id>\", approve: false, \
             feedback: \"<why>\"}})`.",
        );
        crate::history_sync::history_push_and_emit(
            history,
            Self::raw_reminder_message(coco_types::AttachmentKind::QueuedCommand, &body),
            &self.event_tx,
        )
        .await;

        if let Some(tx) = self.event_tx.as_ref() {
            for (_idx, req) in &pending {
                let params = coco_types::PlanApprovalRequestedParams {
                    request_id: req.request_id.clone(),
                    from: req.from.clone(),
                    plan_file_path: Some(req.plan_file_path.clone()),
                    plan_content: req.plan_content.clone(),
                };
                let _ = tx
                    .send(coco_types::CoreEvent::Protocol(
                        coco_types::ServerNotification::PlanApprovalRequested(params),
                    ))
                    .await;
            }
        }

        for (idx, _) in &pending {
            let _ = mailbox.mark_read(agent, team, *idx).await;
        }
    }

    /// Build a bare `<system-reminder>`-wrapped meta user message from
    /// raw text. Shared by the swarm pollers — caller supplies the
    /// [`AttachmentKind`](coco_types::AttachmentKind) for classification.
    fn raw_reminder_message(kind: coco_types::AttachmentKind, text: &str) -> Message {
        Message::Attachment(AttachmentMessage::api(
            kind,
            LlmMessage::user_text(wrap_in_system_reminder(text)),
        ))
    }
}

#[cfg(test)]
#[path = "plan_mode_reminder.test.rs"]
mod tests;
