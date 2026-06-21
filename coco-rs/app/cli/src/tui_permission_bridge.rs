//! TUI permission bridge — drives the permission prompt from a
//! `ToolPermissionBridge::request_permission` call.
//!
//! ## Why
//!
//! Without an installed bridge, the engine's `permission_controller`
//! treats `PermissionDecision::Ask` as "auto-allow" (legacy headless
//! fallback at `permission_controller.rs:100-107`). For interactive
//! TUI users that's the wrong default — Ask should prompt, not pass.
//!
//! This module wires the loop:
//!
//! ```text
//!  engine                       TUI state                user
//!    │                              │                      │
//!    │ request_permission()         │                      │
//!    │ ─ insert oneshot in pending  │                      │
//!    │ ─ emit ApprovalRequired ────>│ Permission prompt    │
//!    │   await oneshot              │ ─────────────────────│
//!    │                              │ <── Approve / Deny ──│
//!    │                              │ UserCommand::Approval
//!    │                              │      Response ──┐    │
//!    │                              │                 ▼    │
//!    │             tui_runner: pop pending oneshot, send   │
//!    │ <─ Approved / Rejected ──────┘                      │
//! ```
//!
//! ## Pieces
//!
//! - [`PendingApprovals`]: shared `Arc<RwLock<HashMap<request_id,
//!   oneshot::Sender>>>` between the bridge (writer) and tui_runner
//!   (reader). Constructed once at TUI startup.
//! - [`TuiPermissionBridge`]: implements `ToolPermissionBridge`. Each
//!   `request_permission` allocates a oneshot, stores the sender in
//!   the pending map, emits `ApprovalRequired` onto the TUI event
//!   channel, and awaits the receiver.
//! - [`resolve_pending`]: tui_runner calls this when
//!   `UserCommand::ApprovalResponse` arrives.
//!
//! ## Cross-mode contract
//!
//! Worker subagents (AgentTool spawns) inherit the leader's bridge
//! via `wire_engine`. So a worker's tool deny in TUI mode prompts the
//! leader's prompt automatically — no per-spawn install needed.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use coco_config::SettingSource;
use coco_query::CoreEvent;
use coco_tool_runtime::{
    ToolPermissionBridge, ToolPermissionDecision, ToolPermissionRequest, ToolPermissionResolution,
};
use coco_types::PendingPermissionGuard;
use coco_types::TuiOnlyEvent;
use tokio::sync::{RwLock, mpsc, oneshot};
use tracing::warn;

use crate::session_runtime::SessionRuntime;

/// One pending approval: the oneshot sender for the resolution + the
/// RAII guard that keeps `ToolAppState.pending_permission_count`
/// incremented while this entry is live. When the entry is removed
/// (via `resolve_pending` or the bridge's drop-cleanup path), the
/// guard drops, decrementing the counter exactly once. Lock-free.
pub struct PendingApprovalEntry {
    pub sender: oneshot::Sender<ToolPermissionResolution>,
    /// `None` for entries created before the runtime weak-ref is
    /// installed (tests / very-early-bootstrap). Production runtimes
    /// always have `Some` because `set_notification_runtime` lands
    /// before any tool can fire `request_permission`.
    pub _guard: Option<PendingPermissionGuard>,
}

/// Shared sender side of pending approvals — keyed by `request_id` so
/// `resolve_pending` can route the matching response back.
pub type PendingApprovals = Arc<RwLock<HashMap<String, PendingApprovalEntry>>>;

/// Build a fresh empty pending map. Hand the same `Arc` to
/// [`TuiPermissionBridge::new`] AND the tui_runner's
/// `UserCommand::ApprovalResponse` arm.
pub fn new_pending_map() -> PendingApprovals {
    Arc::new(RwLock::new(HashMap::new()))
}

/// Bridge implementation for the interactive TUI.
///
/// Holds a clone of the TUI's notification channel (so it can emit
/// `TuiOnlyEvent::ApprovalRequired`) and a clone of the pending
/// oneshot map (so `resolve_pending` can complete the await).
pub struct TuiPermissionBridge {
    notification_tx: mpsc::Sender<CoreEvent>,
    pending: PendingApprovals,
    /// Late-bound `Weak<SessionRuntime>` used to fire the
    /// `Notification` hook when an `Ask` permission lands in front
    /// of the user. Set by
    /// [`Self::set_notification_runtime`] from `tui_runner` after
    /// `SessionRuntime::build` returns. Weak avoids extending the
    /// runtime's lifetime through the bridge.
    notification_runtime: RwLock<Option<Weak<SessionRuntime>>>,
}

impl TuiPermissionBridge {
    pub fn new(notification_tx: mpsc::Sender<CoreEvent>, pending: PendingApprovals) -> Self {
        Self {
            notification_tx,
            pending,
            notification_runtime: RwLock::new(None),
        }
    }

    /// Install the runtime weak-ref used to fire `Notification` hooks
    /// when prompting the user. Call once after `SessionRuntime::build`
    /// returns. Safe to skip — bridge degrades to no hook fire.
    pub async fn set_notification_runtime(&self, weak: Weak<SessionRuntime>) {
        *self.notification_runtime.write().await = Some(weak);
    }

    /// Resolve the `Arc<AtomicU32>` counter on
    /// `ToolAppState.pending_permission_count` via the late-bound
    /// runtime Weak. Returns `None` when the runtime hasn't been
    /// bound yet (test fixtures / startup race) — caller treats that
    /// as "no counter, skip the increment" so prompt-suggestion
    /// suppression degrades gracefully instead of panicking.
    async fn pending_permission_counter(
        &self,
    ) -> Option<std::sync::Arc<std::sync::atomic::AtomicU32>> {
        let runtime = self
            .notification_runtime
            .read()
            .await
            .as_ref()
            .and_then(Weak::upgrade)?;
        let snap = runtime.app_state.read().await;
        Some(snap.pending_permission_count.clone())
    }

    async fn show_always_allow_options(&self) -> bool {
        let Some(runtime) = self
            .notification_runtime
            .read()
            .await
            .as_ref()
            .and_then(Weak::upgrade)
        else {
            return true;
        };
        settings_allow_always_allow_options(&runtime.runtime_config.settings)
    }

    /// Generate an on-demand LLM risk explanation for a pending permission
    /// prompt. Delegates to
    /// [`SessionRuntime::explain_permission_risk`] (the single home for the
    /// explainer call) via the late-bound runtime Weak; returns `None` when the
    /// runtime isn't bound (tests / early bootstrap). The interactive Ctrl+E
    /// path in `tui_runner` calls the `SessionRuntime` method directly.
    pub async fn explain_risk(
        &self,
        params: coco_permissions::ExplainerParams<'_>,
    ) -> Option<coco_types::PermissionExplanation> {
        let runtime = self
            .notification_runtime
            .read()
            .await
            .as_ref()
            .and_then(Weak::upgrade)?;
        runtime.explain_permission_risk(params).await
    }
}

pub fn settings_allow_always_allow_options(settings: &coco_config::SettingsWithSource) -> bool {
    !settings
        .per_source
        .get(&SettingSource::Policy)
        .and_then(|raw| {
            raw.pointer("/permissions/allowManagedPermissionRulesOnly")
                .or_else(|| raw.pointer("/permissions/allow_managed_permission_rules_only"))
        })
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

#[async_trait]
impl ToolPermissionBridge for TuiPermissionBridge {
    async fn request_permission(
        &self,
        mut request: ToolPermissionRequest,
    ) -> Result<ToolPermissionResolution, String> {
        crate::leader_permission::enrich_in_process_worker_badge(&mut request);

        // Step 1: register the oneshot in the pending map BEFORE
        // emitting the event. Reverse order risks a fast-path race
        // where the user clicks Approve before the entry exists and
        // the resolver finds nothing to send to.
        //
        // Acquire a `PendingPermissionGuard` here so the entry's
        // lifetime is the canonical signal for "is the user staring
        // at a permission prompt?". The guard drops when the entry
        // is removed from the map (resolve_pending path) OR when the
        // bridge's cleanup branches below remove it on channel close.
        // Counter Arc is fetched via the late-bound Weak<SessionRuntime>
        // — `None` only in test / very-early-bootstrap paths where
        // the runtime hasn't been bound yet.
        let pending_perm_counter = self.pending_permission_counter().await;
        let guard = pending_perm_counter.map(PendingPermissionGuard::acquire);
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.write().await;
            pending.insert(
                request.id.clone(),
                PendingApprovalEntry {
                    sender: tx,
                    _guard: guard,
                },
            );
        }

        // Fire the Notification hook before the prompt is shown so
        // user-defined notifiers run. Best-effort — no runtime installed
        // (e.g. tests) leaves the hook unfired.
        if let Some(runtime) = self
            .notification_runtime
            .read()
            .await
            .as_ref()
            .and_then(Weak::upgrade)
        {
            let title = format!("Permission request: {}", request.tool_name);
            runtime
                .fire_notification_hooks(
                    "permission_prompt",
                    "Claude Code needs your permission to use a tool",
                    Some(&title),
                )
                .await;
        }

        // Step 2: emit the right prompt event onto the TUI channel.
        //
        // AskUserQuestion gets a dedicated rich prompt (Question UI:
        // multi-question, multiSelect, preview, notes). All other tools get
        // the generic Allow / Deny `Permission` prompt.
        //
        // Both paths land back here via the same `pending` oneshot
        // and `UserCommand::ApprovalResponse` channel — the only
        // difference is how the TUI collects the user's input
        // before resolving.
        let event = if request.tool_name == coco_types::ToolName::AskUserQuestion.as_str() {
            CoreEvent::Tui(TuiOnlyEvent::QuestionAsked {
                request_id: request.id.clone(),
                input: request.input.clone(),
            })
        } else {
            let is_exit_plan_mode =
                request.tool_name == coco_types::ToolName::ExitPlanMode.as_str();
            let choices = request.choices.clone();
            let show_always_allow = !is_exit_plan_mode && self.show_always_allow_options().await;
            CoreEvent::Tui(TuiOnlyEvent::ApprovalRequired {
                request_id: request.id.clone(),
                tool_name: request.tool_name.clone(),
                description: request.description.clone(),
                display_input: if is_exit_plan_mode {
                    coco_types::PermissionDisplayInput::Empty
                } else {
                    coco_tui::tool_display::permission_display_input(
                        &request.tool_name,
                        &request.input,
                    )
                },
                show_always_allow,
                choices,
                detail: request.detail.clone(),
                permission_suggestions: request.suggestions.clone(),
                // Carry the raw input so classic read permissions can derive
                // path-scoped "always allow" updates.
                original_input: Some(request.input.clone()),
                cwd: request.cwd.clone(),
                worker_badge: request.worker_badge.clone(),
            })
        };
        if let Err(e) = self.notification_tx.send(event).await {
            // Channel closed → the TUI is shutting down. Pull the
            // pending entry back so we don't leak the oneshot, and
            // bail out closed.
            self.pending.write().await.remove(&request.id);
            return Err(format!("TUI notification channel closed: {e}"));
        }

        // Step 3: await the user's decision (or cancellation).
        match rx.await {
            Ok(resolution) => Ok(resolution),
            Err(_) => {
                // Sender dropped without sending — the pending entry
                // may still be there if the TUI exited without
                // resolving. Best-effort cleanup.
                self.pending.write().await.remove(&request.id);
                Err("Permission response channel closed".into())
            }
        }
    }
}

/// Called by tui_runner when `UserCommand::ApprovalResponse` arrives.
/// Pops the matching oneshot and sends the resolution. Returns `true`
/// when the request_id matched a pending entry, `false` otherwise
/// (stale response after the bridge dropped the sender).
///
/// `permission_updates` are forwarded into
/// [`ToolPermissionResolution::applied_updates`] so audit/logging
/// downstream of the bridge sees what the user authorized. Persistence
/// (settings.json writes) and live engine_config mutation are
/// performed by the consumer (`tui_runner::ApprovalResponse` arm)
/// before this fn is called — by the time the resolution lands on
/// the bridge the rules are already effective.
///
/// `updated_input` carries a user-supplied rewrite of the tool input
/// (e.g. `AskUserQuestion` answers). When `Some`, downstream
/// (`PermissionController::resolve` → `tool_call_preparer`) substitutes
/// it for the original input before invoking the tool.
///
/// `resolution_detail` carries trusted tool-specific approval metadata
/// without rewriting the tool input.
///
/// `content_blocks` carries optional image attachments (etc.) the user
/// pasted alongside the answer. Today the TUI doesn't have a paste-into-
/// question gesture so callers pass `None`; the bridge plumbing is in
/// place so SDK clients (which already ship the field via
/// `ApprovalResolveParams.content_blocks`) flow through unchanged.
#[allow(clippy::too_many_arguments)]
pub async fn resolve_pending(
    pending: &PendingApprovals,
    request_id: &str,
    approved: bool,
    feedback: Option<String>,
    permission_updates: Vec<coco_types::PermissionUpdate>,
    updated_input: Option<serde_json::Value>,
    resolution_detail: Option<coco_types::PermissionResolutionDetail>,
    content_blocks: Option<Vec<serde_json::Value>>,
) -> bool {
    let entry = take_pending(pending, request_id).await;
    let Some(entry) = entry else {
        warn!(%request_id, "ApprovalResponse for unknown request_id (stale or already resolved)");
        return false;
    };
    send_resolution(
        entry,
        approved,
        feedback,
        permission_updates,
        updated_input,
        resolution_detail,
        content_blocks,
    )
}

/// Remove a pending approval without sending a resolution yet.
///
/// Callers that need side effects before unblocking the engine use this
/// to validate the request id first, then apply updates, then call
/// [`send_resolution`]. This prevents stale approval responses from
/// mutating permission state after a request was cancelled.
pub async fn take_pending(
    pending: &PendingApprovals,
    request_id: &str,
) -> Option<PendingApprovalEntry> {
    // Removing the entry drops its `_guard` when the returned entry is
    // later consumed, decrementing pending_permission_count exactly once.
    pending.write().await.remove(request_id)
}

pub fn send_resolution(
    entry: PendingApprovalEntry,
    approved: bool,
    feedback: Option<String>,
    permission_updates: Vec<coco_types::PermissionUpdate>,
    updated_input: Option<serde_json::Value>,
    resolution_detail: Option<coco_types::PermissionResolutionDetail>,
    content_blocks: Option<Vec<serde_json::Value>>,
) -> bool {
    let resolution = ToolPermissionResolution {
        decision: if approved {
            ToolPermissionDecision::Approved
        } else {
            ToolPermissionDecision::Rejected
        },
        feedback,
        applied_updates: permission_updates,
        updated_input,
        content_blocks,
        detail: resolution_detail,
    };
    entry.sender.send(resolution).is_ok()
}

#[cfg(test)]
#[path = "tui_permission_bridge.test.rs"]
mod tests;
