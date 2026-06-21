//! Tests for the cross-process teammate inbox pump's serialization
//! handshake (`inject_and_wait`) — the novel concurrency logic. Mailbox
//! priority/filtering is covered in `coordinator::runner_loop`
//! (`select_mailbox_prompt`); framing in `coordinator::teammate`.

use super::*;

const FRAMED: &str = "<teammate_message teammate_id=\"team-lead\">\nhi\n</teammate_message>";

fn extract_user_message_id(cmd: UserCommand) -> String {
    match cmd {
        UserCommand::SubmitInput {
            user_message_id,
            content,
            ..
        } => {
            assert!(
                content.contains("teammate_message"),
                "pump must inject framed content, got: {content}"
            );
            user_message_id
        }
        other => panic!("expected SubmitInput, got {other:?}"),
    }
}

#[tokio::test]
async fn inject_and_wait_releases_only_on_own_turn_id() {
    let (command_tx, mut command_rx) = mpsc::channel::<UserCommand>(8);
    let (turn_done_tx, turn_done_rx) = mpsc::channel::<String>(8);
    let cancel = CancellationToken::new();
    let cancel_task = cancel.clone();

    let handle = tokio::spawn(async move {
        let mut rx = turn_done_rx;
        inject_and_wait(&command_tx, &mut rx, &cancel_task, FRAMED.to_string()).await
    });

    let id = extract_user_message_id(command_rx.recv().await.unwrap());

    // A foreign turn completing (human typing in the pane / a drained slash
    // turn) must NOT release the pump.
    turn_done_tx
        .send("some-other-turn".to_string())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(
        !handle.is_finished(),
        "pump released on a foreign turn id — would let drain_active_turn cancel its live turn"
    );

    // Its own turn id releases it.
    turn_done_tx.send(id).await.unwrap();
    assert_eq!(handle.await.unwrap(), Some(()));
}

#[tokio::test]
async fn inject_and_wait_exits_on_cancel() {
    let (command_tx, mut command_rx) = mpsc::channel::<UserCommand>(8);
    let (_turn_done_tx, turn_done_rx) = mpsc::channel::<String>(8);
    let cancel = CancellationToken::new();
    let cancel_task = cancel.clone();

    let handle = tokio::spawn(async move {
        let mut rx = turn_done_rx;
        inject_and_wait(&command_tx, &mut rx, &cancel_task, FRAMED.to_string()).await
    });

    // Drain the injected command so the send succeeds, then cancel while the
    // pump is blocked waiting for completion.
    let _ = extract_user_message_id(command_rx.recv().await.unwrap());
    cancel.cancel();
    assert_eq!(handle.await.unwrap(), None);
}

#[tokio::test]
async fn inject_and_wait_exits_when_handshake_sender_dropped() {
    let (command_tx, mut command_rx) = mpsc::channel::<UserCommand>(8);
    let (turn_done_tx, turn_done_rx) = mpsc::channel::<String>(8);
    let cancel = CancellationToken::new();
    let cancel_task = cancel.clone();

    let handle = tokio::spawn(async move {
        let mut rx = turn_done_rx;
        inject_and_wait(&command_tx, &mut rx, &cancel_task, FRAMED.to_string()).await
    });

    let _ = extract_user_message_id(command_rx.recv().await.unwrap());
    // Driver gone: its handshake sender drops → recv() yields None → exit.
    drop(turn_done_tx);
    assert_eq!(handle.await.unwrap(), None);
}

#[tokio::test]
async fn inject_and_wait_exits_when_command_receiver_dropped() {
    let (command_tx, command_rx) = mpsc::channel::<UserCommand>(8);
    let (_turn_done_tx, mut turn_done_rx) = mpsc::channel::<String>(8);
    let cancel = CancellationToken::new();

    // Driver gone before the inject: the send fails immediately.
    drop(command_rx);
    let result = inject_and_wait(&command_tx, &mut turn_done_rx, &cancel, FRAMED.to_string()).await;
    assert_eq!(result, None);
}

#[test]
fn control_mode_set_maps_to_set_permission_mode() {
    // gap 8: a leader ModeSetRequest applies as a live SetPermissionMode.
    let msg = mailbox::ProtocolMessage::ModeSetRequest {
        mode: coco_types::PermissionMode::Plan,
        from: TEAM_LEAD_NAME.to_string(),
    };
    match control_message_to_command(&msg) {
        Some(UserCommand::SetPermissionMode { mode }) => {
            assert_eq!(mode, coco_types::PermissionMode::Plan);
        }
        other => panic!("expected SetPermissionMode, got {other:?}"),
    }
}

#[test]
fn control_other_message_maps_to_none() {
    // A ShutdownRequest is a prompt-path message, not a control message —
    // the control drain must ignore it (leaving it for `scan_next_prompt`).
    let msg = mailbox::ProtocolMessage::ShutdownRequest {
        request_id: "s1".to_string(),
        from: TEAM_LEAD_NAME.to_string(),
        reason: None,
        timestamp: "t".to_string(),
    };
    assert!(control_message_to_command(&msg).is_none());
}

// ── L1: hermetic pump integration tests (real on-disk mailbox) ──
//
// Drive `drain_control_tick` / `scan_tick` against a real file mailbox
// isolated to a tempdir via `COCO_TEAMS_DIR` (the gap-8 L0 infra). This
// exercises the actual file IPC the pump uses end-to-end, not an in-memory
// stub. nextest runs each test in its own process (env-isolated); the async
// `ENV_LOCK` additionally serializes these under bare `cargo test`.

use std::sync::LazyLock;

static ENV_LOCK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Removes `COCO_TEAMS_DIR` on drop so a panicking test can't leak the
/// override into a sibling (belt-and-braces over `ENV_LOCK` + nextest's
/// per-process isolation).
struct TeamsDirGuard;
impl Drop for TeamsDirGuard {
    fn drop(&mut self) {
        // SAFETY: only these env-mutating pump tests touch this var, all under
        // `ENV_LOCK`; nextest isolates per process.
        unsafe { std::env::remove_var("COCO_TEAMS_DIR") };
    }
}

fn set_teams_dir(path: &std::path::Path) -> TeamsDirGuard {
    // SAFETY: serialized via `ENV_LOCK`; see `TeamsDirGuard::drop`.
    unsafe { std::env::set_var("COCO_TEAMS_DIR", path) };
    TeamsDirGuard
}

fn ident(team: &str, name: &str) -> TeammateIdentity {
    TeammateIdentity {
        agent_id: format!("{name}@{team}"),
        agent_name: name.to_string(),
        team_name: team.to_string(),
        color: None,
        plan_mode_required: false,
    }
}

fn leader_msg(text: String) -> mailbox::TeammateMessage {
    mailbox::TeammateMessage {
        from: TEAM_LEAD_NAME.to_string(),
        text,
        timestamp: "t".to_string(),
        read: false,
        color: None,
        summary: None,
    }
}

#[tokio::test]
async fn l0_teams_base_dir_honors_coco_teams_dir_override() {
    // L0: `COCO_TEAMS_DIR` relocates the teams/mailbox base — the single
    // resolution point all mailbox/team-file paths route through.
    let _g = ENV_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let _restore = set_teams_dir(dir.path());
    assert_eq!(
        coco_coordinator::team_file::teams_base_dir(),
        dir.path().to_path_buf(),
        "teams_base_dir must return the COCO_TEAMS_DIR override verbatim"
    );
}

#[tokio::test]
async fn drain_applies_mode_set_against_real_mailbox() {
    let _g = ENV_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let _restore = set_teams_dir(dir.path());
    let id = ident("t", "worker");

    // Leader writes a ModeSetRequest into the teammate's mailbox.
    mailbox::write_to_mailbox(
        &id.agent_name,
        leader_msg(mailbox::create_mode_set_request(
            coco_types::PermissionMode::Plan,
            TEAM_LEAD_NAME,
        )),
        &id.team_name,
    )
    .unwrap();

    let (tx, mut rx) = mpsc::channel::<UserCommand>(8);
    let live_rules = Arc::new(RwLock::new(Vec::new()));
    drain_control_tick(&id, &tx, &live_rules).await;

    match rx.try_recv() {
        Ok(UserCommand::SetPermissionMode { mode }) => {
            assert_eq!(mode, coco_types::PermissionMode::Plan);
        }
        other => panic!("expected SetPermissionMode, got {other:?}"),
    }
    let msgs = mailbox::read_mailbox(&id.agent_name, &id.team_name).unwrap();
    assert!(
        msgs.iter().all(|m| m.read),
        "the consumed control message must be marked read"
    );
}

#[tokio::test]
async fn scan_frames_plain_peer_message_from_real_mailbox() {
    let _g = ENV_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let _restore = set_teams_dir(dir.path());
    let id = ident("t", "worker");

    mailbox::write_to_mailbox(
        &id.agent_name,
        mailbox::TeammateMessage {
            from: "peer".to_string(),
            text: "investigate the bug".to_string(),
            timestamp: "t".to_string(),
            read: false,
            color: None,
            summary: Some("bug".to_string()),
        },
        &id.team_name,
    )
    .unwrap();

    let framed = scan_tick(&id).await.expect("a framed prompt");
    assert!(framed.contains("teammate_message"), "got: {framed}");
    assert!(framed.contains("investigate the bug"), "got: {framed}");
}

#[tokio::test]
async fn scan_prioritizes_shutdown_over_plain_in_real_mailbox() {
    let _g = ENV_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let _restore = set_teams_dir(dir.path());
    let id = ident("t", "worker");

    // A normal peer message AND a leader shutdown request in the same inbox.
    mailbox::write_to_mailbox(
        &id.agent_name,
        mailbox::TeammateMessage {
            from: "peer".to_string(),
            text: "later work".to_string(),
            timestamp: "t".to_string(),
            read: false,
            color: None,
            summary: None,
        },
        &id.team_name,
    )
    .unwrap();
    mailbox::send_shutdown_request(
        &id.agent_name,
        &id.team_name,
        TEAM_LEAD_NAME,
        Some("disband"),
    )
    .unwrap();

    let framed = scan_tick(&id).await.expect("a framed prompt");
    // Priority: shutdown > team-lead > peer.
    assert!(
        framed.contains("shutdown request"),
        "shutdown must outrank the peer message; got: {framed}"
    );
}
