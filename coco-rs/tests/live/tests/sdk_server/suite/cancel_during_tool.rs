//! Cancel during tool execution: kick off a turn that runs `sleep 20`
//! via the Bash tool, fire `turn/interrupt` after the tool starts but
//! before it can finish, and verify the wire stream ends with
//! `turn/interrupted` (or `turn/failed`) — never `turn/completed`.
//!
//! Engine wiring: `CancellationToken` is created per turn and threaded
//! into every tool execution. `BashTool` spawns the shell child with
//! tokio's `process::Command`; on cancel the child is dropped, which
//! sends SIGKILL on Unix. The streaming executor then sees the
//! cancellation and short-circuits the iteration.
//!
//! Distinct from `test_sdk_turn_mid_flight_interrupt_*` which fires
//! interrupt during the model stream (before any tool runs). This test
//! catches the engine *inside* a tool execution.

use anyhow::Result;
use anyhow::anyhow;
use coco_cli::sdk_server::SdkTransport;

use crate::sdk_server::harness::build_live_server;
use crate::sdk_server::harness::drive_until_response;
use crate::sdk_server::harness::is_turn_terminal_method;
use crate::sdk_server::harness::req;
use crate::sdk_server::harness::send_initialize;
use crate::sdk_server::harness::send_session_start;

use coco_types::ClientRequestMethod;
use coco_types::JsonRpcMessage;
use coco_types::NotificationMethod;

pub async fn run(provider: &str, model: &str) -> Result<()> {
    let server = build_live_server(provider, model).await?;
    let _ = send_initialize(&server).await?;
    let _ = send_session_start(&server).await?;

    // Kick off the long-running tool turn. 20s is long enough to give
    // us time to interrupt; short enough to bound the test if cancel
    // somehow doesn't reach the child.
    server
        .client
        .send(req(
            5,
            ClientRequestMethod::TurnStart.as_str(),
            serde_json::json!({
                "prompt": "Use the Bash tool to run exactly: sleep 20 && echo finished. \
                           After it returns, reply with: done.",
            }),
        ))
        .await
        .map_err(|e| anyhow!("send turn/start: {e:?}"))?;
    let (_resp, mut notifs) =
        drive_until_response(&server.client, 5, std::time::Duration::from_secs(60)).await?;

    // Drain until we see an `item/started` (or `item/updated`) whose
    // `details.type == "command_execution"` — that's the wire shape for a
    // Bash tool call. `ItemStarted` fires when the tool is queued (input
    // parsed) and `ItemUpdated` fires the moment execution actually begins.
    // Either is fine for our purposes: we just need to know a Bash item
    // exists before we send the interrupt; the subprocess will be
    // sleep(20)-ing well within the wall-clock budget.
    let started_at = tokio::time::Instant::now();
    let deadline = started_at + std::time::Duration::from_secs(60);
    let mut bash_started = false;
    while !bash_started && tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, server.client.recv()).await {
            Ok(Ok(Some(JsonRpcMessage::Notification(n)))) => {
                let is_command_item = (n.method == NotificationMethod::ItemStarted.as_str()
                    || n.method == NotificationMethod::ItemUpdated.as_str())
                    && n.params
                        .get("item")
                        .and_then(|i| i.get("details"))
                        .and_then(|d| d.get("type"))
                        .and_then(|v| v.as_str())
                        == Some("command_execution");
                notifs.push(n);
                if is_command_item {
                    bash_started = true;
                }
            }
            Ok(Ok(Some(_))) => {}
            Ok(Ok(None)) | Ok(Err(_)) | Err(_) => break,
        }
    }
    assert!(
        bash_started,
        "Bash tool never started; observed methods={:?}",
        notifs.iter().map(|n| n.method.as_str()).collect::<Vec<_>>(),
    );

    // Fire interrupt while the bash child is sleeping.
    server
        .client
        .send(req(
            6,
            ClientRequestMethod::TurnInterrupt.as_str(),
            serde_json::json!({}),
        ))
        .await
        .map_err(|e| anyhow!("send turn/interrupt: {e:?}"))?;

    // Wait for terminal. Total wall-clock from turn/start should stay
    // well under the sleep duration if cancellation worked (without
    // cancel we'd burn the full 20s).
    let term_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    while !notifs.iter().any(|n| is_turn_terminal_method(&n.method)) {
        let remaining = term_deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            let methods: Vec<&str> = notifs.iter().map(|n| n.method.as_str()).collect();
            return Err(anyhow!(
                "interrupted-during-tool turn never reached terminal; observed methods={methods:?}"
            ));
        }
        match tokio::time::timeout(remaining, server.client.recv()).await {
            Ok(Ok(Some(JsonRpcMessage::Notification(n)))) => notifs.push(n),
            Ok(Ok(Some(_))) => {}
            Ok(Ok(None)) | Ok(Err(_)) | Err(_) => break,
        }
    }
    let last_terminal = notifs
        .iter()
        .rev()
        .find(|n| is_turn_terminal_method(&n.method))
        .cloned();
    let terminal = last_terminal.expect("expected a final turn/ended terminator");
    assert_eq!(
        terminal.method,
        NotificationMethod::TurnEnded.as_str(),
        "cancel-during-tool's last terminator must be turn/ended"
    );
    // Outcome must be `interrupted` — never `failed` (the engine_session
    // Err path now suppresses the `Failed` emit when cancel was the
    // cause, so cancellation-induced bails surface only as the runner's
    // `Interrupted` terminator), never `completed` (would mean the
    // cancel did not propagate at all), and never `max_turns_reached`
    // / `budget_exhausted` (wrong reason for this scenario).
    let outcome_kind = terminal
        .params
        .get("outcome")
        .and_then(|o| o.get("kind"))
        .and_then(|k| k.as_str())
        .unwrap_or("");
    assert_eq!(
        outcome_kind, "interrupted",
        "cancel-during-tool outcome must be `interrupted`; got `{outcome_kind}`. \
         If this asserts as `failed` again, the engine_session Err path \
         likely regressed and is emitting Failed for cancel-induced Err."
    );

    // Wall-clock check: if cancel didn't propagate to the bash child
    // we'd burn the full sleep. 18s is a generous upper bound that's
    // still well below 20s.
    let elapsed = started_at.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(18),
        "cancel-during-tool took too long (cancellation likely didn't reach the bash child): \
         {elapsed:?}"
    );

    server.shutdown().await;
    Ok(())
}
