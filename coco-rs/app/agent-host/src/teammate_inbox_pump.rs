//! Cross-process teammate inbox→turn pump (gap 1, the keystone).
//!
//! A cross-process (tmux / iTerm2) teammate is a full `coco` TUI process
//! launched with `COCO_AGENT_*` env, so
//! [`coco_coordinator::identity::resolve_teammate_identity`] returns `Some`.
//! Its initial task, every leader/peer message, and shutdown requests are
//! written to its own file mailbox (`pane_executor.rs` writes the initial
//! prompt there with `from = team-lead`). Nothing on the teammate side reads
//! that mailbox, so the teammate boots idle forever — the gap this pump
//! closes.
//!
//! The in-process teammate has a driver loop
//! ([`coco_coordinator::runner_loop::run_in_process_teammate`]) that calls
//! `wait_for_next_prompt_or_shutdown` and submits each message to its engine.
//! The cross-process teammate already runs the TUI agent driver
//! (`tui_runner::run_agent_driver`), so this pump reuses the SHARED priority
//! scan ([`coco_coordinator::runner_loop::scan_next_prompt`]) and injects each
//! result as a `UserCommand::SubmitInput`, then blocks until THAT turn
//! completes before scanning again.
//!
//! ## Serialization (why the completion handshake exists)
//! `SubmitInput`'s `drain_active_turn(Wait)` CANCELS any in-flight turn
//! (last-write-wins). Injecting a second prompt while the first turn runs
//! would abort the teammate's current work. So the pump injects exactly one
//! prompt and blocks on a turn-completion handshake (`turn_done_rx`) before
//! the next scan. The handshake is keyed by the pump's own `user_message_id`:
//! `run_agent_driver` fires the completed turn's id and the pump ignores
//! foreign ids (a human typing in the pane, a drained slash turn), so it only
//! ever releases on ITS OWN turn — it can never release early and let
//! `drain_active_turn` cancel its live turn.
//!
//! ## Why always-frame
//! Every injected prompt is wrapped via
//! [`coco_coordinator::teammate::format_as_teammate_message`], so the content
//! always starts with `<teammate_message …>` and is never empty. That
//! guarantees it can never hit the `SubmitInput` arm's empty-content or
//! slash-command `continue` early-returns — both of which skip turn-spawn and
//! would wedge the handshake forever. A spawned turn always drops its
//! completion guard (even on cancel/panic), so the pump can never deadlock.
//!
//! ## Lifecycle
//! Fire-and-forget, spawned only for a teammate session with
//! `Feature::AgentTeams`. It exits cleanly on any of: the `cancel` token
//! (fired by `tui_runner` after `app.run()` returns), a failed
//! `command_tx.send` (driver gone), or `turn_done_rx` closing (driver dropped
//! its sender). On exit it drops its `command_tx` clone so the driver's
//! `command_rx` can drain to `None` and the process can exit — without this
//! the held clone would block `driver_handle.await` forever.
//!
//! ## Control messages (gap 8)
//! Before each prompt scan, [`drain_control_tick`] applies leader-pushed
//! controls (the cross-process analog of the in-process runner's
//! `drain_control_messages`): a `ModeSetRequest` injects
//! `UserCommand::SetPermissionMode`, and a `TeamPermissionUpdate` extends the
//! teammate's shared live-rules `Arc` (seeded at boot from the team's
//! `team_allowed_paths` and installed on the engine config). Both apply
//! without spawning a turn.
//!
//! ## Out of scope (tracked follow-ups)
//! - Pane teardown on ShutdownRequest (gap 6): the request is delivered as a
//!   turn so the teammate can wrap up; the leader owns the pane kill via the
//!   `ShutdownApproved` round-trip.
//! - Teammate→leader idle/result reporting after each turn.

use std::sync::Arc;
use std::time::Duration;

use coco_types::PermissionRule;
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use coco_coordinator::constants::TEAM_LEAD_NAME;
use coco_coordinator::mailbox;
use coco_coordinator::runner_loop::{WaitResult, scan_next_prompt};
use coco_coordinator::teammate::format_as_teammate_message;
use coco_coordinator::types::TeammateIdentity;
use coco_tui::UserCommand;

/// Mailbox poll cadence (500 ms) — this pump is the cross-process analog of
/// the in-process runner loop.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Spawn the cross-process teammate inbox pump. The caller guarantees this
/// process is a teammate (identity resolved) with agent-teams enabled.
///
/// - `command_tx`: a clone of the TUI command channel (must be cloned before
///   `App::new` consumes the original).
/// - `turn_done_rx`: receives the `user_message_id` of each completed
///   top-level turn (fired by `run_agent_driver`).
/// - `cancel`: fired by the runner after `app.run()` returns so the pump drops
///   `command_tx` and the driver can shut down.
/// - `live_permission_rules`: the same `Arc` installed on the teammate's
///   engine config at boot (seeded from `team_allowed_paths`); the pump
///   extends it when a leader `TeamPermissionUpdate` arrives.
pub fn spawn(
    identity: TeammateIdentity,
    command_tx: mpsc::Sender<UserCommand>,
    turn_done_rx: mpsc::Receiver<String>,
    cancel: CancellationToken,
    live_permission_rules: Arc<RwLock<Vec<PermissionRule>>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        run(
            identity,
            command_tx,
            turn_done_rx,
            cancel,
            live_permission_rules,
        )
        .await;
    })
}

async fn run(
    identity: TeammateIdentity,
    command_tx: mpsc::Sender<UserCommand>,
    mut turn_done_rx: mpsc::Receiver<String>,
    cancel: CancellationToken,
    live_permission_rules: Arc<RwLock<Vec<PermissionRule>>>,
) {
    loop {
        // Apply any pending leader control messages (mode / permission rules)
        // FIRST so the next turn runs under the right policy (gap 8).
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = drain_control_tick(&identity, &command_tx, &live_permission_rules) => {}
        }
        let framed = tokio::select! {
            _ = cancel.cancelled() => return,
            framed = scan_tick(&identity) => framed,
        };
        match framed {
            // Nothing actionable — wait a poll interval (cancellable).
            None => {
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = tokio::time::sleep(POLL_INTERVAL) => {}
                }
            }
            Some(framed) => {
                if inject_and_wait(&command_tx, &mut turn_done_rx, &cancel, framed)
                    .await
                    .is_none()
                {
                    // Driver gone or cancelled — drop command_tx and exit.
                    return;
                }
            }
        }
    }
}

/// One mailbox scan. Returns the framed prompt text to inject, or `None` when
/// no plain-text message / unclaimed task is pending. Task-list claiming is
/// disabled (`None`) — a cross-process teammate's initial task and all
/// follow-ups arrive as mailbox messages, written by the leader's pane
/// executor and `send_message` path.
async fn scan_tick(identity: &TeammateIdentity) -> Option<String> {
    let messages =
        mailbox::read_mailbox(&identity.agent_name, &identity.team_name).unwrap_or_default();
    match scan_next_prompt(identity, None, &messages).await? {
        // Defensive: the shared scan never returns `Aborted`.
        WaitResult::Aborted => None,
        WaitResult::ShutdownRequest { original_text } => Some(format_as_teammate_message(
            TEAM_LEAD_NAME,
            &original_text,
            None,
            Some("shutdown request"),
        )),
        // Always frame (no raw `from == "user"` path): keeps content off the
        // SubmitInput slash/empty `continue` early-returns.
        WaitResult::NewMessage {
            message,
            from,
            color,
            summary,
        } => Some(format_as_teammate_message(
            &from,
            &message,
            color.as_deref(),
            summary.as_deref(),
        )),
    }
}

/// Drain leader→teammate control messages (gap 8). A `ModeSetRequest` is
/// applied by injecting `UserCommand::SetPermissionMode` — the cross-process
/// analog of the in-process runner's `drain_control_messages`, reusing the
/// session's existing live permission-mode seam rather than a parallel control
/// state. Fire-and-forget: `SetPermissionMode` updates config + app_state
/// without spawning a turn, so no completion handshake is needed.
///
/// A `TeamPermissionUpdate` (rule push) extends the shared live-rules `Arc`
/// in place — the cross-process analog of the in-process runner's
/// `team_permission_rules` store.
async fn drain_control_tick(
    identity: &TeammateIdentity,
    command_tx: &mpsc::Sender<UserCommand>,
    live_rules: &Arc<RwLock<Vec<PermissionRule>>>,
) {
    let messages =
        mailbox::read_mailbox(&identity.agent_name, &identity.team_name).unwrap_or_default();
    for (i, msg) in messages.iter().enumerate() {
        if msg.read
            || msg.from != TEAM_LEAD_NAME
            || !mailbox::is_structured_protocol_message(&msg.text)
        {
            continue;
        }
        let Some(parsed) = mailbox::parse_protocol_message(&msg.text) else {
            continue;
        };
        let applied = if let Some(cmd) = control_message_to_command(&parsed) {
            command_tx.send(cmd).await.is_ok()
        } else if let mailbox::ProtocolMessage::TeamPermissionUpdate {
            permission_update, ..
        } = &parsed
        {
            let rules = permission_update.clone().into_permission_rules();
            if !rules.is_empty() {
                live_rules.write().await.extend(rules);
            }
            true
        } else {
            false
        };
        if applied {
            let _ = mailbox::mark_message_as_read_by_index(
                &identity.agent_name,
                &identity.team_name,
                i,
            );
        }
    }
}

/// Pure mapping from a leader control message to the `UserCommand` that
/// applies it to this teammate's live session. `ModeSetRequest` →
/// `SetPermissionMode`; everything else is `None` (left unread). Split out
/// so the dispatch decision is unit-testable without a file mailbox.
fn control_message_to_command(parsed: &mailbox::ProtocolMessage) -> Option<UserCommand> {
    match parsed {
        mailbox::ProtocolMessage::ModeSetRequest { mode, .. } => {
            Some(UserCommand::SetPermissionMode { mode: *mode })
        }
        _ => None,
    }
}

/// Inject one framed prompt as a TUI turn and block until THAT turn completes,
/// ignoring foreign turns. Returns `Some(())` when the injected turn finished
/// (keep pumping), or `None` when the pump must exit (cancelled, or the driver
/// is gone).
async fn inject_and_wait(
    command_tx: &mpsc::Sender<UserCommand>,
    turn_done_rx: &mut mpsc::Receiver<String>,
    cancel: &CancellationToken,
    framed: String,
) -> Option<()> {
    let id = uuid::Uuid::new_v4().to_string();
    let cmd = UserCommand::SubmitInput {
        user_message_id: id.clone(),
        content: framed,
        display_text: None,
        images: Vec::new(),
    };

    // Async send → backpressures on a momentarily-full channel; an error means
    // the driver's receiver is gone.
    tokio::select! {
        _ = cancel.cancelled() => return None,
        sent = command_tx.send(cmd) => sent.ok()?,
    }

    // Block until our own turn completes. `turn_done_rx` carries the
    // `user_message_id` of every completed top-level turn; ignore any that
    // aren't ours (human-typed / drained-slash turns in the pane).
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return None,
            done = turn_done_rx.recv() => match done {
                Some(done_id) if done_id == id => return Some(()),
                Some(_) => continue,
                None => return None, // driver dropped the sender
            },
        }
    }
}

#[cfg(test)]
#[path = "teammate_inbox_pump.test.rs"]
mod tests;
