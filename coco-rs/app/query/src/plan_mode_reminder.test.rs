//! Tests for the per-turn side-effect driver.
//!
//! Reminder-emission tests (plan/auto/todo/task/etc.) now live in
//! `coco-system-reminder::generators/*.test.rs`. This file covers only the
//! per-turn side effects [`PlanModeReminder::turn_start_side_effects_only`]
//! owns: mode reconciliation, teammate approval polling, and leader
//! pending-approvals injection.

use super::PlanModeReminder;
use coco_messages::Message;
use coco_messages::MessageHistory;
use coco_messages::UserContent;
use coco_types::PermissionMode;
use coco_types::ToolAppState;
use std::sync::Arc;
use tempfile::tempdir;
use tokio::sync::RwLock;

fn text_of(msg: &Message) -> Option<String> {
    match msg {
        Message::Attachment(a) => match a.as_api_message() {
            Some(coco_messages::LlmMessage::User { content, .. }) => {
                content.iter().find_map(|c| match c {
                    UserContent::Text(t) => Some(t.text.clone()),
                    _ => None,
                })
            }
            _ => None,
        },
        _ => None,
    }
}

fn history_texts(history: &MessageHistory) -> Vec<String> {
    history.iter().filter_map(|m| text_of(m)).collect()
}

// ── Mode reconciliation ──────────────────────────────────────────────

#[tokio::test]
async fn unannounced_plan_exit_via_shift_tab_sets_flags() {
    // Last turn was Plan, this turn is Default (Shift+Tab). Reconcile
    // observes the change via `last_permission_mode` and sets the flags
    // the orchestrator reads this turn.
    let app_state = Arc::new(RwLock::new(ToolAppState {
        last_permission_mode: Some(PermissionMode::Plan),
        ..Default::default()
    }));
    let tmp = tempdir().unwrap();
    let mut r = PlanModeReminder::new(
        PermissionMode::Default,
        Some("s1".into()),
        None,
        Some(tmp.path().to_path_buf()),
        Some(app_state.clone()),
    );
    let mut h = MessageHistory::new();
    r.turn_start_side_effects_only(&mut h).await;

    // Side-effects-only never writes reminders into history.
    assert!(h.is_empty(), "no emission from side-effects-only");

    let guard = app_state.read().await;
    assert!(guard.has_exited_plan_mode);
    assert!(guard.needs_plan_mode_exit_attachment);
    assert_eq!(guard.last_permission_mode, Some(PermissionMode::Default));
}

#[tokio::test]
async fn unannounced_plan_entry_via_shift_tab_clears_stale_exit_flag() {
    // Plan → Default → Plan via Shift+Tab: the intermediate Default leg
    // set `needs_plan_mode_exit_attachment`; we must clear it when we
    // land back in Plan so the orchestrator doesn't emit a stale banner.
    let app_state = Arc::new(RwLock::new(ToolAppState {
        last_permission_mode: Some(PermissionMode::Default),
        needs_plan_mode_exit_attachment: true,
        ..Default::default()
    }));
    let tmp = tempdir().unwrap();
    let mut r = PlanModeReminder::new(
        PermissionMode::Plan,
        Some("s1".into()),
        None,
        Some(tmp.path().to_path_buf()),
        Some(app_state.clone()),
    );
    let mut h = MessageHistory::new();
    r.turn_start_side_effects_only(&mut h).await;

    let guard = app_state.read().await;
    assert!(
        !guard.needs_plan_mode_exit_attachment,
        "plan re-entry clears the stale exit-attachment flag"
    );
}

#[tokio::test]
async fn reconcile_auto_to_default_sets_auto_mode_exit_flag() {
    // Shift+Tab Auto → Default: reconcile observes the change via
    // `last_permission_mode` and sets the auto-exit flag the orchestrator
    // reads this turn.
    let app_state = Arc::new(RwLock::new(ToolAppState {
        last_permission_mode: Some(PermissionMode::Auto),
        ..Default::default()
    }));
    let tmp = tempdir().unwrap();
    let mut r = PlanModeReminder::new(
        PermissionMode::Default,
        Some("s1".into()),
        None,
        Some(tmp.path().to_path_buf()),
        Some(app_state.clone()),
    );
    let mut h = MessageHistory::new();
    r.turn_start_side_effects_only(&mut h).await;

    let guard = app_state.read().await;
    assert!(guard.needs_auto_mode_exit_attachment);
}

#[tokio::test]
async fn reentering_auto_before_banner_clears_stale_flag() {
    // Rapid Auto → Default → Auto: exit banner is stale — we're back in
    // Auto. Reconcile clears the pending flag.
    let app_state = Arc::new(RwLock::new(ToolAppState {
        last_permission_mode: Some(PermissionMode::Default),
        needs_auto_mode_exit_attachment: true,
        ..Default::default()
    }));
    let tmp = tempdir().unwrap();
    let mut r = PlanModeReminder::new(
        PermissionMode::Auto,
        Some("s1".into()),
        None,
        Some(tmp.path().to_path_buf()),
        Some(app_state.clone()),
    );
    let mut h = MessageHistory::new();
    r.turn_start_side_effects_only(&mut h).await;
    assert!(
        !app_state.read().await.needs_auto_mode_exit_attachment,
        "re-entering Auto must clear the stale exit flag"
    );
}

// ── Teammate approval polling (F3) ───────────────────────────────────

/// In-memory mailbox handle so tests can inject messages + verify reads.
#[derive(Default)]
struct FakeMailbox {
    inboxes: std::sync::Mutex<
        std::collections::HashMap<(String, String), Vec<coco_tool_runtime::InboxMessage>>,
    >,
    marked: std::sync::Mutex<Vec<(String, String, usize)>>,
    next_index: std::sync::Mutex<std::collections::HashMap<(String, String), usize>>,
}

impl FakeMailbox {
    fn push(&self, agent: &str, team: &str, text: &str, from: &str) {
        let key = (agent.to_string(), team.to_string());
        let mut idx_guard = self.next_index.lock().unwrap();
        let idx = idx_guard.entry(key.clone()).or_insert(0);
        let this_idx = *idx;
        *idx += 1;
        drop(idx_guard);
        self.inboxes.lock().unwrap().entry(key).or_default().push(
            coco_tool_runtime::InboxMessage {
                index: this_idx,
                from: from.into(),
                text: text.into(),
                timestamp: "t".into(),
            },
        );
    }
}

#[async_trait::async_trait]
impl coco_tool_runtime::MailboxHandle for FakeMailbox {
    async fn write_to_mailbox(
        &self,
        _recipient: &str,
        _team: &str,
        _m: coco_tool_runtime::MailboxEnvelope,
    ) -> Result<(), coco_error::BoxedError> {
        Ok(())
    }
    async fn read_unread(
        &self,
        agent: &str,
        team: &str,
    ) -> Result<Vec<coco_tool_runtime::InboxMessage>, coco_error::BoxedError> {
        Ok(self
            .inboxes
            .lock()
            .unwrap()
            .get(&(agent.to_string(), team.to_string()))
            .cloned()
            .unwrap_or_default())
    }
    async fn mark_read(
        &self,
        agent: &str,
        team: &str,
        index: usize,
    ) -> Result<(), coco_error::BoxedError> {
        self.marked
            .lock()
            .unwrap()
            .push((agent.into(), team.into(), index));
        let mut guard = self.inboxes.lock().unwrap();
        if let Some(msgs) = guard.get_mut(&(agent.to_string(), team.to_string())) {
            msgs.retain(|m| m.index != index);
        }
        Ok(())
    }
}

#[tokio::test]
async fn teammate_polls_approval_and_injects_approved_reminder() {
    let app_state = Arc::new(RwLock::new(ToolAppState {
        awaiting_plan_approval_request_id: Some("plan_approval-alice-team-a-deadbeef".into()),
        awaiting_plan_approval: true,
        ..Default::default()
    }));
    let mailbox = Arc::new(FakeMailbox::default());
    mailbox.push(
        "alice",
        "team-a",
        r#"{"type":"plan_approval_response","request_id":"plan_approval-alice-team-a-deadbeef","approved":true,"permission_mode":"accept_edits"}"#,
        "team-lead",
    );

    let tmp = tempdir().unwrap();
    let mut r = PlanModeReminder::new(
        PermissionMode::Plan,
        Some("s1".into()),
        None,
        Some(tmp.path().to_path_buf()),
        Some(app_state.clone()),
    )
    .with_mailbox(mailbox.clone(), "alice".into(), "team-a".into(), true);

    let mut h = MessageHistory::new();
    r.turn_start_side_effects_only(&mut h).await;

    let texts = history_texts(&h);
    assert!(texts[0].contains("## Plan Approved"));
    assert!(texts[0].contains("acceptEdits"));

    let guard = app_state.read().await;
    assert!(!guard.awaiting_plan_approval);
    assert!(guard.awaiting_plan_approval_request_id.is_none());
    assert_eq!(
        guard.last_permission_mode,
        Some(PermissionMode::AcceptEdits)
    );

    let marked = mailbox.marked.lock().unwrap();
    assert_eq!(marked.len(), 1);
    assert_eq!(marked[0].0, "alice");
}

#[tokio::test]
async fn teammate_polls_approval_and_injects_rejected_reminder_with_feedback() {
    let app_state = Arc::new(RwLock::new(ToolAppState {
        awaiting_plan_approval_request_id: Some("plan_approval-alice-team-a-cafebabe".into()),
        awaiting_plan_approval: true,
        ..Default::default()
    }));
    let mailbox = Arc::new(FakeMailbox::default());
    mailbox.push(
        "alice",
        "team-a",
        r#"{"type":"plan_approval_response","request_id":"plan_approval-alice-team-a-cafebabe","approved":false,"feedback":"refine the security section"}"#,
        "team-lead",
    );

    let tmp = tempdir().unwrap();
    let mut r = PlanModeReminder::new(
        PermissionMode::Plan,
        Some("s1".into()),
        None,
        Some(tmp.path().to_path_buf()),
        Some(app_state.clone()),
    )
    .with_mailbox(mailbox, "alice".into(), "team-a".into(), true);

    let mut h = MessageHistory::new();
    r.turn_start_side_effects_only(&mut h).await;

    let texts = history_texts(&h);
    assert!(texts[0].contains("## Plan Rejected"));
    assert!(texts[0].contains("refine the security section"));
}

#[tokio::test]
async fn teammate_ignores_unrelated_response_ids() {
    let app_state = Arc::new(RwLock::new(ToolAppState {
        awaiting_plan_approval_request_id: Some("plan_approval-alice-team-a-mine".into()),
        awaiting_plan_approval: true,
        ..Default::default()
    }));
    let mailbox = Arc::new(FakeMailbox::default());
    mailbox.push(
        "alice",
        "team-a",
        r#"{"type":"plan_approval_response","request_id":"plan_approval-bob-team-a-theirs","approved":true}"#,
        "team-lead",
    );

    let tmp = tempdir().unwrap();
    let mut r = PlanModeReminder::new(
        PermissionMode::Plan,
        Some("s1".into()),
        None,
        Some(tmp.path().to_path_buf()),
        Some(app_state.clone()),
    )
    .with_mailbox(mailbox, "alice".into(), "team-a".into(), true);

    let mut h = MessageHistory::new();
    r.turn_start_side_effects_only(&mut h).await;
    let texts = history_texts(&h);
    assert!(
        !texts.iter().any(|t| t.contains("## Plan Approved"))
            && !texts.iter().any(|t| t.contains("## Plan Rejected")),
        "unrelated request_id must not trigger approval consumption"
    );

    let guard = app_state.read().await;
    assert!(guard.awaiting_plan_approval);
}

#[tokio::test]
async fn team_lead_sees_pending_approvals_attachment() {
    let mailbox = Arc::new(FakeMailbox::default());
    mailbox.push(
        "team-lead",
        "team-a",
        r##"{"type":"plan_approval_request","from":"alice","planFilePath":"/tmp/alice.md","planContent":"# Alice's plan","requestId":"req-1"}"##,
        "alice",
    );
    mailbox.push(
        "team-lead",
        "team-a",
        r##"{"type":"plan_approval_request","from":"bob","planFilePath":"/tmp/bob.md","planContent":"# Bob's plan","requestId":"req-2"}"##,
        "bob",
    );

    let tmp = tempdir().unwrap();
    let mut r = PlanModeReminder::new(
        PermissionMode::Default,
        Some("leader-session".into()),
        None,
        Some(tmp.path().to_path_buf()),
        None,
    )
    .with_mailbox(mailbox.clone(), "team-lead".into(), "team-a".into(), false);

    let mut h = MessageHistory::new();
    r.turn_start_side_effects_only(&mut h).await;

    let texts = history_texts(&h);
    assert_eq!(texts.len(), 1);
    let t = &texts[0];
    assert!(t.contains("## Pending Plan Approvals"));
    assert!(t.contains("alice"));
    assert!(t.contains("bob"));
    assert!(t.contains("req-1"));
    assert!(t.contains("req-2"));
    assert!(t.contains("Alice's plan"));
    assert!(t.contains("Bob's plan"));
    let marked = mailbox.marked.lock().unwrap();
    assert_eq!(marked.len(), 2);
}

#[tokio::test]
async fn non_leader_agent_does_not_inject_pending_attachment() {
    let mailbox = Arc::new(FakeMailbox::default());
    mailbox.push(
        "alice",
        "team-a",
        r##"{"type":"plan_approval_request","from":"bob","planFilePath":"/tmp/bob.md","planContent":"# plan","requestId":"req-1"}"##,
        "bob",
    );
    let tmp = tempdir().unwrap();
    let mut r = PlanModeReminder::new(
        PermissionMode::Default,
        Some("s1".into()),
        None,
        Some(tmp.path().to_path_buf()),
        None,
    )
    .with_mailbox(mailbox, "alice".into(), "team-a".into(), false);

    let mut h = MessageHistory::new();
    r.turn_start_side_effects_only(&mut h).await;
    let texts = history_texts(&h);
    assert!(
        !texts.iter().any(|t| t.contains("Pending Plan Approvals")),
        "only the team-lead identity scans pending approvals"
    );
}
