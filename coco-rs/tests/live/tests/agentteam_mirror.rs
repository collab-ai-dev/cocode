//! In-process teammate runner — protocol boundary smoke test.
//!
//! The pre-refactor version of this file exercised a parallel
//! `InProcessTeammateTaskState` mirror that the coordinator used to
//! double-write alongside the canonical `TaskManager` row. The
//! task-storage unification (see `docs/coco-rs/task-storage-refactor.md`)
//! deleted that mirror — the runner now writes only via the
//! `TaskHandle::update_teammate_task` registry path.
//!
//! What this file still covers, byte-faithful to TS:
//!
//! - Mailbox protocol-boundary acceptance: the `parse_protocol_message`
//!   discriminator rejects legacy snake_case shapes and bad enum values
//!   so a teammate cannot silently observe a malformed control envelope.
//!
//! The active-query / interrupt / plan-mode scenarios that used to live
//! here now belong in `tasks/running.test.rs` (where the canonical
//! TaskManager is exercised) and `coordinator/agent_handle/mod.test.rs`
//! (where the trait-driven teammate registration is exercised). Tests
//! over the runner_loop's mailbox draining will be re-added once the
//! TaskRuntime fixture lands in the live harness.

use coco_coordinator::mailbox;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agentteam_mailbox_protocol_boundary() {
    protocol_boundary_rejects_non_ts_shapes();
}

fn protocol_boundary_rejects_non_ts_shapes() {
    let legacy_team_update = r#"{"type":"team_permission_update","permission_update":{"type":"add_rules","rules":[{"tool_name":"Edit","rule_content":"/repo/**"}],"behavior":"allow","destination":"session"},"directory_path":"/repo","tool_name":"Edit"}"#;
    assert!(
        mailbox::parse_protocol_message(legacy_team_update).is_none(),
        "mailbox must reject pre-refactor snake_case team permission updates"
    );

    let bad_mode = r#"{"type":"mode_set_request","mode":"not-a-mode","from":"team-lead"}"#;
    assert!(
        mailbox::parse_protocol_message(bad_mode).is_none(),
        "mode_set_request.mode must stay typed as PermissionMode"
    );

    let bad_permission_response =
        r#"{"type":"permission_response","request_id":"r1","subtype":"maybe"}"#;
    assert!(
        mailbox::parse_protocol_message(bad_permission_response).is_none(),
        "permission_response.subtype must stay a success/error union"
    );
}
