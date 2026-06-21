//! `session/archive` emits a `session/result` notification carrying
//! the aggregated stats for the session.
//!
//! Engine wiring: `handle_session_archive` (`session.rs:382-`)
//! synthesizes a `SessionResultParams` from the active
//! `SessionHandle.stats` (turn count, duration, token usage, cost,
//! per-model breakdown) and emits one final
//! `ServerNotification::SessionResult` on the outbound channel before
//! clearing the slot. The notification is the SDK's "session is
//! done" terminator.
//!
//! TS parity: `cli/print.ts runHeadless()` emits the same aggregate
//! at the end of the session.

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
use coco_types::JsonRpcMessage;
use coco_types::NotificationMethod;

pub async fn run(provider: &str, model: &str) -> Result<()> {
    let server = build_live_server(provider, model).await?;
    let _ = send_initialize(&server).await?;
    let (start_resp, _start_notifs) = send_session_start(&server).await?;
    let session_id = start_resp
        .get("session_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("session/start response missing session_id; resp={start_resp}"))?
        .to_string();

    // One turn so the aggregate carries non-zero `total_turns`.
    let (_r1, n1) = send_turn(&server, 2, "Reply with one word: ok").await?;
    assert!(
        n1.iter().any(|n| is_turn_terminal_method(&n.method)),
        "first turn must produce a terminal notification"
    );

    // Send `session/archive`. The handler will:
    //   1. Cancel any in-flight turn (none here).
    //   2. Build a SessionResultParams aggregate.
    //   3. Emit `session/result` on the outbound channel.
    //   4. Clear the session slot.
    server
        .client
        .send(req(
            10,
            ClientRequestMethod::SessionArchive.as_str(),
            serde_json::json!({ "session_id": session_id }),
        ))
        .await
        .map_err(|e| anyhow!("send session/archive: {e:?}"))?;
    let (_archive_resp, archive_notifs) =
        drive_until_response(&server.client, 10, std::time::Duration::from_secs(20)).await?;

    // The session/result event may arrive in `archive_notifs` (if it
    // landed before the response ack) OR right after — drain a bit
    // more to be sure.
    let mut all_notifs = archive_notifs;
    let drain_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    while tokio::time::Instant::now() < drain_deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(200), server.client.recv())
            .await
        {
            Ok(Ok(Some(JsonRpcMessage::Notification(n)))) => all_notifs.push(n),
            Ok(Ok(Some(_))) => {}
            Ok(Ok(None)) => break,
            Ok(Err(_)) => break,
            Err(_) => {} // poll timeout; loop
        }
    }

    let result_notif = all_notifs
        .iter()
        .find(|n| n.method == NotificationMethod::SessionResult.as_str())
        .ok_or_else(|| {
            let methods: Vec<&str> = all_notifs.iter().map(|n| n.method.as_str()).collect();
            anyhow!("session/result not emitted by archive; observed methods={methods:?}")
        })?;
    let total_turns = result_notif
        .params
        .get("total_turns")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| {
            anyhow!(
                "session/result missing total_turns; params={:?}",
                result_notif.params
            )
        })?;
    assert!(
        total_turns >= 1,
        "session/result.total_turns must reflect the executed turn; got {total_turns}"
    );
    let result_session_id = result_notif
        .params
        .get("session_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("session/result missing session_id"))?;
    assert_eq!(
        result_session_id, session_id,
        "session/result.session_id must match the archived session"
    );

    server.shutdown().await;
    Ok(())
}
