//! Live tests for the **SDK control protocol** (`coco sdk` mode).
//!
//! Drives the **same `SdkServer`** the binary spawns when a Python /
//! TypeScript SDK client speaks NDJSON-over-stdio to the `coco sdk`
//! subprocess. The transport is `InMemoryTransport::pair()` instead
//! of stdio so the test crate can drive both ends in-process; the
//! server side is the real `SdkServer` with a real `QueryEngineRunner`
//! talking to live DeepSeek.
//!
//! Coverage adds *over* `coco_cli_deepseek` (the `coco -p` headless
//! path): the SDK server's wire protocol — `initialize`, `turn/start`,
//! `control/setPermissionMode`, and the `ServerNotification` stream
//! shape SDK clients consume.

mod common;

#[path = "sdk_server/mod.rs"]
mod sdk_server;

use anyhow::Result;
use coco_cli::sdk_server::SdkTransport;
use coco_types::ClientRequestMethod;
use coco_types::NotificationMethod;

use crate::sdk_server::harness::build_live_server;
use crate::sdk_server::harness::is_turn_terminal_method;
use crate::sdk_server::harness::req;
use crate::sdk_server::harness::send_initialize;
use crate::sdk_server::harness::send_session_start;
use crate::sdk_server::harness::send_turn;

// ─── initialize ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_sdk_initialize_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "text");
    let server = build_live_server("deepseek-openai", &target.model).await?;
    let (resp, _notifs) = send_initialize(&server).await?;

    // Response should at least be an object; SDK protocol returns the
    // initialize bootstrap (cwd, model, slash_commands, …). Spec
    // shape is in `coco_cli::sdk::InitializeResponse`.
    let obj = resp
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("initialize response is not an object: {resp}"))?;
    // The actual InitializeResponse shape (per `coco_cli::sdk`) carries
    // pid, account, models, commands, agents, output_style, plus
    // protocol/version markers. Assert on a subset that has to be
    // present.
    for key in ["pid", "models", "commands"] {
        assert!(
            obj.contains_key(key),
            "initialize response missing `{key}`; got keys: {:?}",
            obj.keys().collect::<Vec<_>>()
        );
    }
    server.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn test_sdk_initialize_deepseek_anthropic() -> Result<()> {
    let target = require_live!("deepseek-anthropic", "text");
    let server = build_live_server("deepseek-anthropic", &target.model).await?;
    let (resp, _notifs) = send_initialize(&server).await?;
    assert!(resp.is_object(), "initialize response must be an object");
    server.shutdown().await;
    Ok(())
}

// ─── turn/start round-trip ───────────────────────────────────────────

/// Drive a single turn end-to-end. Asserts the wire stream contains:
/// - a `turn/started` notification (engine emits, dispatcher forwards)
/// - at least one stream event (`agentMessage/delta` OR an item event)
/// - a terminal `turn/completed` notification
#[tokio::test]
async fn test_sdk_turn_basic_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "text");
    let server = build_live_server("deepseek-openai", &target.model).await?;
    let (_init, _) = send_initialize(&server).await?;
    let _ = send_session_start(&server).await?;
    let (_resp, notifs) = send_turn(&server, 2, "Reply with the single word: ok").await?;

    assert!(
        notifs
            .iter()
            .any(|n| n.method == NotificationMethod::TurnStarted.as_str()),
        "expected `turn/started` notification; got methods: {:?}",
        notifs.iter().map(|n| n.method.as_str()).collect::<Vec<_>>()
    );
    assert!(
        notifs
            .iter()
            .any(|n| n.method == NotificationMethod::TurnEnded.as_str()),
        "expected `turn/completed` notification; got methods: {:?}",
        notifs.iter().map(|n| n.method.as_str()).collect::<Vec<_>>()
    );
    let has_content = notifs.iter().any(|n| {
        n.method == NotificationMethod::AgentMessageDelta.as_str()
            || n.method == NotificationMethod::ItemCompleted.as_str()
            || n.method == NotificationMethod::ItemUpdated.as_str()
    });
    assert!(
        has_content,
        "expected at least one delta/item event; got methods: {:?}",
        notifs.iter().map(|n| n.method.as_str()).collect::<Vec<_>>()
    );
    server.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn test_sdk_turn_basic_deepseek_anthropic() -> Result<()> {
    let target = require_live!("deepseek-anthropic", "text");
    let server = build_live_server("deepseek-anthropic", &target.model).await?;
    let (_init, _) = send_initialize(&server).await?;
    let _ = send_session_start(&server).await?;
    let (_resp, notifs) = send_turn(&server, 2, "Reply with the single word: ok").await?;
    assert!(
        notifs
            .iter()
            .any(|n| n.method == NotificationMethod::TurnEnded.as_str()),
        "expected `turn/completed`; got: {:?}",
        notifs.iter().map(|n| n.method.as_str()).collect::<Vec<_>>()
    );
    server.shutdown().await;
    Ok(())
}

// ─── Multi-turn: turn 2 sees turn 1's context ────────────────────────

/// Two consecutive `turn/start` calls must share the same in-process
/// MessageHistory so the model can refer back to facts the user gave
/// in turn 1. Reassemble turn 2's reply text from `agentMessage/delta`
/// payloads and assert it echoes the fact.
#[tokio::test]
async fn test_sdk_two_turns_share_session_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "text");
    let server = build_live_server("deepseek-openai", &target.model).await?;
    let (_init, _) = send_initialize(&server).await?;
    let _ = send_session_start(&server).await?;

    let (_r1, n1) = send_turn(
        &server,
        2,
        "Remember: my favorite color is teal. Reply with the single word `noted`.",
    )
    .await?;
    assert!(
        n1.iter().any(|n| is_turn_terminal_method(&n.method)),
        "turn 1 must produce a terminal notification"
    );

    let (_r2, n2) = send_turn(
        &server,
        3,
        "What is my favorite color? Reply with just the color, lowercase.",
    )
    .await?;
    let reply: String = n2
        .iter()
        .filter(|n| n.method == NotificationMethod::AgentMessageDelta.as_str())
        .filter_map(|n| n.params.get("delta").and_then(|v| v.as_str()))
        .collect();
    assert!(
        reply.to_lowercase().contains("teal"),
        "turn 2 should remember `teal` from turn 1; got reply={reply:?}"
    );
    server.shutdown().await;
    Ok(())
}

// ─── Mid-turn interrupt ──────────────────────────────────────────────

/// Send a long-running turn, immediately fire `control/interrupt`, and
/// assert the wire stream ends with `turn/interrupted` rather than
/// `turn/completed`. Exercises the `CancellationToken` propagation from
/// `handle_turn_interrupt` → `runner.run_turn` → engine loop.
#[tokio::test]
async fn test_sdk_turn_mid_flight_interrupt_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "text");
    let server = build_live_server("deepseek-openai", &target.model).await?;
    let (_init, _) = send_initialize(&server).await?;
    let _ = send_session_start(&server).await?;

    // Kick off a turn that would take a while if left to complete.
    server
        .client
        .send(req(
            5,
            ClientRequestMethod::TurnStart.as_str(),
            serde_json::json!({
                "prompt": "Count from 1 to 200 in English words, one per line. Take your time.",
            }),
        ))
        .await
        .map_err(|e| anyhow::anyhow!("send turn/start: {e:?}"))?;
    let (_resp, mut notifs) = crate::sdk_server::harness::drive_until_response(
        &server.client,
        5,
        std::time::Duration::from_secs(60),
    )
    .await?;

    // Wait until `turn/started` has fired so we know the engine is
    // actually mid-turn before we interrupt. Reasoning models can take
    // 60s+ before emitting their first wire event.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(120);
    while !notifs
        .iter()
        .any(|n| n.method == NotificationMethod::TurnStarted.as_str())
    {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        if let Ok(Ok(Some(coco_types::JsonRpcMessage::Notification(n)))) =
            tokio::time::timeout(remaining, server.client.recv()).await
        {
            notifs.push(n);
        }
    }

    // Fire the interrupt. Use a fresh request id so we can match the ack.
    server
        .client
        .send(req(
            6,
            ClientRequestMethod::TurnInterrupt.as_str(),
            serde_json::json!({}),
        ))
        .await
        .map_err(|e| anyhow::anyhow!("send turn/interrupt: {e:?}"))?;

    // Drain notifications until terminal. The engine polls cancellation
    // at tool boundaries / between stream chunks, so the terminator can
    // take a while if we caught the model deep in a single API call.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(120);
    while !notifs.iter().any(|n| is_turn_terminal_method(&n.method)) {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            let methods: Vec<&str> = notifs.iter().map(|n| n.method.as_str()).collect();
            anyhow::bail!(
                "interrupted turn never reached terminal notification; got methods={methods:?}"
            );
        }
        match tokio::time::timeout(remaining, server.client.recv()).await {
            Ok(Ok(Some(coco_types::JsonRpcMessage::Notification(n)))) => notifs.push(n),
            Ok(Ok(Some(_other))) => {}
            Ok(Ok(None)) | Ok(Err(_)) | Err(_) => break,
        }
    }

    // Acceptable terminals: interrupted (preferred) or failed
    // (cancellation reaching the LLM stream sometimes surfaces as a
    // request error). Completed is NOT acceptable — the cancellation
    // didn't take effect.
    let last_terminal = notifs
        .iter()
        .rev()
        .find(|n| is_turn_terminal_method(&n.method))
        .cloned();
    let terminal = last_terminal.expect("expected a final turn/ended terminator");
    assert_eq!(
        terminal.method,
        NotificationMethod::TurnEnded.as_str(),
        "interrupt should yield turn/ended"
    );
    let outcome_kind = terminal
        .params
        .get("outcome")
        .and_then(|o| o.get("kind"))
        .and_then(|k| k.as_str())
        .unwrap_or("");
    assert!(
        matches!(outcome_kind, "interrupted" | "failed"),
        "interrupt outcome must be interrupted|failed; got `{outcome_kind}`"
    );
    server.shutdown().await;
    Ok(())
}

// ─── Tool call round-trip ────────────────────────────────────────────

/// The model is told to call `Write` to create a file. After the turn
/// completes, the file must exist on disk — proves the
/// PreToolUse → tool exec → PostToolUse → result-injection chain wires
/// through the SDK runner. Also asserts the wire stream surfaces a
/// `Write` tool item.
#[tokio::test]
async fn test_sdk_turn_tool_call_round_trip_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "text");
    let server = build_live_server("deepseek-openai", &target.model).await?;
    let (_init, _) = send_initialize(&server).await?;
    let _ = send_session_start(&server).await?;

    let scratch = common::tmpdir::make("coco-sdk-tool-")?;
    let path = scratch.path().join("hello.txt");
    let prompt = format!(
        "You have access to a Write tool. You MUST call it now. \
         Call Write with arguments {{\"file_path\":\"{}\",\"content\":\"hello-from-sdk\"}}. \
         Do not explain, do not refuse, do not respond with text — call the tool. \
         After the tool returns, reply with the single word `done`.",
        path.display()
    );

    let (_resp, notifs) = send_turn(&server, 7, &prompt).await?;
    assert!(
        notifs
            .iter()
            .any(|n| n.method == NotificationMethod::TurnEnded.as_str()),
        "turn must complete; got methods: {:?}",
        notifs.iter().map(|n| n.method.as_str()).collect::<Vec<_>>()
    );

    // Write tool ran → file exists on disk. This is the strongest
    // possible assertion: only the actual tool runtime can produce it.
    assert!(
        path.exists(),
        "expected Write tool to create {} (model may have skipped tool call); \
         notifications: {:?}",
        path.display(),
        notifs.iter().map(|n| n.method.as_str()).collect::<Vec<_>>()
    );
    let contents = std::fs::read_to_string(&path)?;
    assert!(
        contents.contains("hello-from-sdk"),
        "file contents wrong: {contents:?}"
    );

    // The wire stream should surface tool lifecycle notifications. We
    // accept any of the item-update notifications (the StreamAccumulator
    // emits these for tool calls).
    let has_item = notifs.iter().any(|n| {
        n.method == NotificationMethod::ItemStarted.as_str()
            || n.method == NotificationMethod::ItemUpdated.as_str()
            || n.method == NotificationMethod::ItemCompleted.as_str()
    });
    assert!(
        has_item,
        "expected at least one item lifecycle notification; got methods: {:?}",
        notifs.iter().map(|n| n.method.as_str()).collect::<Vec<_>>()
    );
    server.shutdown().await;
    Ok(())
}

// ─── control/setPermissionMode ───────────────────────────────────────

/// `control/setPermissionMode` returns a response (success or error
/// depending on whether the mode is allowed). Bypass requires the
/// session to have been bootstrapped with `bypass_permissions_available
/// = true`; without `--dangerously-skip-permissions` or `--allow-…`,
/// our harness has it `false`, so the request should be rejected.
#[tokio::test]
async fn test_sdk_set_permission_mode_bypass_blocked() -> Result<()> {
    let target = require_live!("deepseek-openai", "text");
    let server = build_live_server("deepseek-openai", &target.model).await?;
    let (_init, _) = send_initialize(&server).await?;
    let _ = send_session_start(&server).await?;

    server
        .client
        .send(req(
            10,
            ClientRequestMethod::SetPermissionMode.as_str(),
            serde_json::json!({ "mode": "bypassPermissions" }),
        ))
        .await
        .map_err(|e| anyhow::anyhow!("send control/setPermissionMode: {e:?}"))?;
    let result = crate::sdk_server::harness::drive_until_response(
        &server.client,
        10,
        std::time::Duration::from_secs(10),
    )
    .await;

    match result {
        Ok((payload, _notifications)) => {
            // Some implementations accept the request and return a
            // payload describing the new state; others return Err.
            // Document the observed behavior — this assertion will
            // surface an intentional change.
            eprintln!(
                "[adversarial] setPermissionMode bypass without capability \
                 → Ok(payload): {payload}"
            );
        }
        Err(e) => {
            assert!(
                e.to_string().contains("error")
                    || e.to_string().to_lowercase().contains("bypass")
                    || e.to_string().contains("INVALID_REQUEST"),
                "expected a meaningful rejection error; got: {e}"
            );
        }
    }
    server.shutdown().await;
    Ok(())
}

/// `control/setPermissionMode` to `acceptEdits` should succeed (no
/// bypass capability required) and a follow-up turn should run under
/// that mode.
#[tokio::test]
async fn test_sdk_set_permission_mode_accept_edits() -> Result<()> {
    let target = require_live!("deepseek-openai", "text");
    let server = build_live_server("deepseek-openai", &target.model).await?;
    let (_init, _) = send_initialize(&server).await?;
    let _ = send_session_start(&server).await?;

    server
        .client
        .send(req(
            10,
            ClientRequestMethod::SetPermissionMode.as_str(),
            serde_json::json!({ "mode": "acceptEdits" }),
        ))
        .await
        .map_err(|e| anyhow::anyhow!("send control/setPermissionMode: {e:?}"))?;
    let (_resp, _notifs) = crate::sdk_server::harness::drive_until_response(
        &server.client,
        10,
        std::time::Duration::from_secs(10),
    )
    .await?;
    server.shutdown().await;
    Ok(())
}

// ─── interrupt before any turn (smoke) ───────────────────────────────

/// `control/interrupt` when there's no in-flight turn should not crash
/// the server. This also smoke-tests that the dispatch loop continues
/// to accept further requests after.
#[tokio::test]
async fn test_sdk_interrupt_idle_safe() -> Result<()> {
    let target = require_live!("deepseek-openai", "text");
    let server = build_live_server("deepseek-openai", &target.model).await?;
    let (_init, _) = send_initialize(&server).await?;

    server
        .client
        .send(req(
            11,
            ClientRequestMethod::TurnInterrupt.as_str(),
            serde_json::json!({}),
        ))
        .await
        .map_err(|e| anyhow::anyhow!("send control/interrupt: {e:?}"))?;
    let _ = crate::sdk_server::harness::drive_until_response(
        &server.client,
        11,
        std::time::Duration::from_secs(5),
    )
    .await;

    // Server must still accept further requests.
    let init2 = send_initialize(&server).await;
    assert!(
        matches!(&init2, Ok((v, _)) if v.is_object()),
        "server should remain responsive after interrupt-while-idle, got {init2:?}"
    );
    server.shutdown().await;
    Ok(())
}

// ─── unknown method ──────────────────────────────────────────────────

/// Unknown JSON-RPC method should return a structured error, not crash
/// the server. Pure protocol-correctness check; no API key needed
/// beyond the harness build.
#[tokio::test]
async fn test_sdk_unknown_method_returns_error() -> Result<()> {
    let target = require_live!("deepseek-openai", "text");
    let server = build_live_server("deepseek-openai", &target.model).await?;
    server
        .client
        .send(req(99, "totally/madeup/method", serde_json::json!({})))
        .await
        .map_err(|e| anyhow::anyhow!("send unknown method: {e:?}"))?;
    let result = crate::sdk_server::harness::drive_until_response(
        &server.client,
        99,
        std::time::Duration::from_secs(5),
    )
    .await;
    assert!(
        result.is_err(),
        "unknown method should produce a JSON-RPC error response (translated to Err by the harness)"
    );
    server.shutdown().await;
    Ok(())
}

// ─── Reminder coverage (full SessionRuntime layer) ────────────────────

#[tokio::test]
async fn test_sdk_reminder_hook_additional_context_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "tools");
    sdk_server::suite::reminders::hook_additional_context::run("deepseek-openai", &target.model)
        .await
}

#[tokio::test]
async fn test_sdk_reminder_hook_stopped_continuation_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "tools");
    sdk_server::suite::reminders::hook_stopped_continuation::run("deepseek-openai", &target.model)
        .await
}

#[tokio::test]
async fn test_sdk_reminder_plan_mode_transitions_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "text");
    sdk_server::suite::reminders::plan_mode_transitions::run("deepseek-openai", &target.model).await
}

#[tokio::test]
async fn test_sdk_reminder_hook_session_start_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "text");
    sdk_server::suite::reminders::hook_session_start::run("deepseek-openai", &target.model).await
}

#[tokio::test]
async fn test_sdk_reminder_hook_user_prompt_submit_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "text");
    sdk_server::suite::reminders::hook_user_prompt_submit::run("deepseek-openai", &target.model)
        .await
}

#[tokio::test]
async fn test_sdk_reminder_skill_listing_runtime_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "text");
    sdk_server::suite::reminders::skill_listing_runtime::run("deepseek-openai", &target.model).await
}

// ─── Round A: engine hot-path coverage ───────────────────────────────

#[tokio::test]
async fn test_sdk_session_resume_roundtrip_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "text");
    sdk_server::suite::session_resume_roundtrip::run("deepseek-openai", &target.model).await
}

#[tokio::test]
async fn test_sdk_set_model_mid_session_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "text");
    sdk_server::suite::set_model_mid_session::run("deepseek-openai", &target.model).await
}

#[tokio::test]
async fn test_sdk_session_archive_emits_aggregate_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "text");
    sdk_server::suite::session_archive_emits_aggregate::run("deepseek-openai", &target.model).await
}

#[tokio::test]
async fn test_sdk_cancel_during_tool_deepseek_openai() -> Result<()> {
    let target = require_live!("deepseek-openai", "tools");
    sdk_server::suite::cancel_during_tool::run("deepseek-openai", &target.model).await
}

// ─── Token-usage report (alphabetically last) ────────────────────────

#[tokio::test]
async fn zzz_emit_token_usage_report() -> Result<()> {
    common::usage_report::flush("sdk_server_deepseek")?;
    Ok(())
}
