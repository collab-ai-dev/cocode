//! `control/setModel` non-disruptive mid-session call.
//!
//! Engine wiring: `handle_set_model` (`runtime.rs:20-43`) updates
//! `SessionHandle.model`; the change takes effect on the *next*
//! `turn/start`. The SDK runner reads `handoff.model` and threads it
//! into `QueryEngineConfig.model_id` for the per-turn engine.
//!
//! Scope of this test: pure protocol-level verification that
//! `control/setModel` returns `Ok` mid-session and the session keeps
//! processing turns afterwards. We deliberately don't cross provider
//! adapters — this path only exercises the control-plane rebind, not a
//! distinct cross-provider runtime setup. That's a follow-up.

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
    let _ = send_session_start(&server).await?;

    // Turn 1.
    let (_r1, n1) = send_turn(&server, 2, "Reply with one word: ok").await?;
    assert!(
        n1.iter().any(|n| is_turn_terminal_method(&n.method)),
        "turn 1 must produce a terminal notification"
    );

    // Re-issue setModel with the same model id. This is the cheapest
    // path through the handler that doesn't require a second provider
    // runtime.
    let model_arg = format!("{provider}/{model}");
    server
        .client
        .send(req(
            10,
            ClientRequestMethod::SetModel.as_str(),
            serde_json::json!({ "model": model_arg }),
        ))
        .await
        .map_err(|e| anyhow!("send control/setModel: {e:?}"))?;
    // drive_until_response returns Err on JSON-RPC error responses, so
    // reaching here means setModel succeeded.
    let (_set_resp, _set_notifs) =
        drive_until_response(&server.client, 10, std::time::Duration::from_secs(15)).await?;

    // Drain stale notifs before next turn.
    while let Ok(Ok(_)) =
        tokio::time::timeout(std::time::Duration::from_millis(50), server.client.recv()).await
    {}

    // Turn 2 — must continue to run cleanly.
    let (_r2, n2) = send_turn(&server, 11, "Reply with one word: ok").await?;
    assert!(
        n2.iter().any(|n| is_turn_terminal_method(&n.method)),
        "post-setModel turn must produce a terminal notification"
    );

    server.shutdown().await;
    Ok(())
}
