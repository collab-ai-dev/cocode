//! Protocol-level test for `session/resume` re-installing a persisted
//! session as the active slot.
//!
//! Engine wiring:
//! - `session/start` creates a `SessionHandle` and persists a session
//!   summary via `SessionManager`.
//! - `session/resume` reads the persisted record back, cancels any
//!   in-flight turn on the previously active slot, and installs a
//!   fresh `SessionHandle` keyed on the same `session_id`. See
//!   `app/cli/src/sdk_server/handlers/session.rs:678-755`.
//!
//! This test asserts: resume returns metadata for the same session_id
//! after a successful `session/start`, and a subsequent `turn/start`
//! runs cleanly on the resumed slot. History restoration is **not**
//! asserted — the resume path documents that it restores metadata
//! only (id/model/cwd); transcript replay is a follow-up.
//!
//! Note on `session/archive`: archive deletes the persisted record,
//! so `archive` → `resume` is unsupported by design. We only test
//! the resume side here.

use anyhow::Result;
use anyhow::anyhow;
use coco_cli::sdk_server::SdkTransport;

use crate::sdk_server::harness::build_live_server;
use crate::sdk_server::harness::drive_until_response;
use crate::sdk_server::harness::is_turn_terminal_method;
use crate::sdk_server::harness::req;
use crate::sdk_server::harness::send_initialize;
use crate::sdk_server::harness::send_session_start;
use crate::sdk_server::harness::send_turn;

use coco_types::ClientRequestMethod;

pub async fn run(provider: &str, model: &str) -> Result<()> {
    let server = build_live_server(provider, model).await?;
    let _ = send_initialize(&server).await?;

    // Capture the session id from the `session/start` response.
    // `drive_until_response` already unwraps `resp.result` to the
    // top level, so `session_id` lives at the value's root.
    let (start_resp, _start_notifs) = send_session_start(&server).await?;
    let session_id = start_resp
        .get("session_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("session/start response missing session_id; resp={start_resp}"))?
        .to_string();

    // One round-trip turn so the session has at least one user message
    // recorded. Required so the resume path has a populated session
    // record to read back.
    let (_r1, n1) = send_turn(&server, 2, "Reply with one word: ok").await?;
    assert!(
        n1.iter().any(|n| is_turn_terminal_method(&n.method)),
        "first turn must produce a terminal notification before resume"
    );

    // Drain any in-flight notifications before issuing session/resume
    // so the next drive_until_response sees its terminator cleanly.
    while let Ok(Ok(_)) =
        tokio::time::timeout(std::time::Duration::from_millis(50), server.client.recv()).await
    {}

    // Resume — replaces the active session slot with a fresh handle
    // bound to the persisted session_id.
    server
        .client
        .send(req(
            11,
            ClientRequestMethod::SessionResume.as_str(),
            serde_json::json!({ "session_id": session_id }),
        ))
        .await
        .map_err(|e| anyhow!("send session/resume: {e:?}"))?;
    let (resume_resp, _resume_notifs) =
        drive_until_response(&server.client, 11, std::time::Duration::from_secs(20)).await?;
    let resumed_id = resume_resp
        .get("session")
        .and_then(|s| s.get("session_id"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            anyhow!("session/resume response missing session.session_id; resp={resume_resp}")
        })?;
    assert_eq!(
        resumed_id, session_id,
        "resumed session_id must match archived id"
    );

    // Drain any stale notifs (e.g. session/started for the resumed slot)
    // before running the next turn so its terminal arrives cleanly.
    while let Ok(Ok(_)) =
        tokio::time::timeout(std::time::Duration::from_millis(50), server.client.recv()).await
    {}

    // A follow-up turn after resume must run end-to-end. The session
    // history starts empty (per resume docstring) so we just assert
    // the turn completes — no continuity claim.
    let (_r2, n2) = send_turn(&server, 12, "Reply with one word: ok").await?;
    assert!(
        n2.iter().any(|n| is_turn_terminal_method(&n.method)),
        "post-resume turn must produce a terminal notification"
    );

    server.shutdown().await;
    Ok(())
}
