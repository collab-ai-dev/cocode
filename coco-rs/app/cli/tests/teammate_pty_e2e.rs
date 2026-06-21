//! L2 of the agent-teams E2E plan (`docs/coco-rs/agentteam-e2e-test-design.md`):
//! spawn the REAL `coco` binary as a cross-process teammate inside a PTY,
//! pointed at a local `wiremock` model, and assert the teammate boots from
//! `COCO_AGENT_*` env, the inbox→turn pump consumes its file mailbox, and it
//! runs one LLM turn carrying the framed prompt (gap 1, end-to-end).
//!
//! Robust assertion: the mock model server receives a `POST /messages` whose
//! body contains the framed prompt we seeded. Per the design's §8 finding, the
//! teammate's *reply text* is NOT auto-routed to the lead mailbox by the
//! cross-process pump, so we assert on the model request (proves
//! mailbox→pump→turn→LLM) rather than on a lead-mailbox reply.
//!
//! **Self-gating — NOT `#[ignore]`d.** Heavy (real binary + full TUI bootstrap
//! in a PTY), but it runs as part of the normal suite wherever it CAN —
//! a unix platform (PTYs) with the `coco` binary built — and auto-skips
//! otherwise (Windows, or the binary unresolved) via [`pty_e2e_binary`]. cargo
//! builds the `coco` binary and sets `CARGO_BIN_EXE_coco` before this
//! integration test, so a normal `cargo nextest run -p coco-cli` exercises it.
//!
//! Load-bearing config detail (design §1): the Anthropic `base_url` MUST be
//! repointed via `providers.json` under `COCO_CONFIG_DIR` — `ANTHROPIC_BASE_URL`
//! is not consulted in coco-rs's provider-resolution path.

use std::collections::HashMap;
use std::time::Duration;
use std::time::Instant;

use coco_coordinator::mailbox;
use coco_utils_pty::TerminalSize;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

const TEAM: &str = "e2e";
const WORKER: &str = "worker";
/// Distinctive token so we can find the framed prompt inside the model
/// request body unambiguously.
const PROMPT_MARKER: &str = "INVESTIGATE_MARKER_42";

/// Whether the real-binary PTY E2E can run here, returning the resolved `coco`
/// binary path (or `None` to auto-skip). PTYs are unix-only, so on non-unix
/// this is a compile-time `None`; on unix it resolves the binary that cargo
/// built + exported via `CARGO_BIN_EXE_coco` (absent ⇒ skip rather than panic).
#[cfg(unix)]
fn pty_e2e_binary() -> Option<std::path::PathBuf> {
    coco_utils_cargo_bin::cargo_bin("coco").ok()
}

#[cfg(not(unix))]
fn pty_e2e_binary() -> Option<std::path::PathBuf> {
    None
}

/// Minimal Anthropic Messages SSE stream: one text block, clean stop. The
/// teammate's turn streams (design §3), so the mock must serve
/// `text/event-stream`.
fn minimal_sse() -> String {
    [
        "event: message_start",
        r#"data: {"type":"message_start","message":{"id":"msg_1","model":"claude-test","usage":{"input_tokens":10},"content":[]}}"#,
        "",
        "event: content_block_start",
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        "",
        "event: content_block_delta",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ack"}}"#,
        "",
        "event: content_block_stop",
        r#"data: {"type":"content_block_stop","index":0}"#,
        "",
        "event: message_delta",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}"#,
        "",
        "event: message_stop",
        r#"data: {"type":"message_stop"}"#,
        "",
        "",
    ]
    .join("\n")
}

#[tokio::test]
async fn teammate_pty_consumes_mailbox_and_runs_turn() {
    // 0. Adaptive gate (replaces `#[ignore]`): skip on non-unix or when the
    //    `coco` binary isn't built, BEFORE any heavy setup.
    let Some(bin) = pty_e2e_binary() else {
        eprintln!(
            "skipping teammate_pty_consumes_mailbox_and_runs_turn: real-binary PTY E2E \
             unsupported here (non-unix, or coco binary unresolved)"
        );
        return;
    };

    // 1. Mock model server (records requests).
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(minimal_sse().into_bytes(), "text/event-stream"),
        )
        .mount(&server)
        .await;

    // 2. Temp config dir: repoint anthropic base_url at the mock + pick a model.
    //    No trailing `/v1` → request path is `/messages`.
    let config_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        config_dir.path().join("providers.json"),
        format!(r#"{{"anthropic":{{"base_url":"{}"}}}}"#, server.uri()),
    )
    .unwrap();
    std::fs::write(
        config_dir.path().join("settings.json"),
        r#"{"models":{"main":"anthropic/claude-haiku-4-5"}}"#,
    )
    .unwrap();

    // 3. Temp teams dir + seed the teammate's mailbox with one prompt. The test
    //    process writes via the coordinator API, so it needs COCO_TEAMS_DIR too.
    //    SAFETY: this integration test runs in its own process (nextest).
    let teams_dir = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("COCO_TEAMS_DIR", teams_dir.path()) };
    mailbox::write_to_mailbox(
        WORKER,
        mailbox::TeammateMessage {
            from: "team-lead".to_string(),
            text: format!("Please {PROMPT_MARKER} the failing test."),
            timestamp: "2026-06-04T00:00:00Z".to_string(),
            read: false,
            color: None,
            summary: Some("initial task".to_string()),
        },
        TEAM,
    )
    .expect("seed teammate mailbox");

    // 4. Spawn the real `coco` teammate in a PTY (`bin` resolved at step 0).
    //    `spawn_process` does `env_clear()`, so pass the full env explicitly.
    let mut env: HashMap<String, String> = HashMap::new();
    for key in ["PATH", "HOME", "USER", "LANG", "TERM"] {
        if let Ok(val) = std::env::var(key) {
            env.insert(key.to_string(), val);
        }
    }
    env.insert(
        "COCO_CONFIG_DIR".to_string(),
        config_dir.path().display().to_string(),
    );
    env.insert(
        "COCO_TEAMS_DIR".to_string(),
        teams_dir.path().display().to_string(),
    );
    env.insert("ANTHROPIC_API_KEY".to_string(), "dummy".to_string());
    env.insert("COCO_AGENT_ID".to_string(), format!("{WORKER}@{TEAM}"));
    env.insert("COCO_AGENT_NAME".to_string(), WORKER.to_string());
    env.insert("COCO_TEAM_NAME".to_string(), TEAM.to_string());
    env.insert("COCO_FEATURE_AGENT_TEAMS".to_string(), "1".to_string());

    let spawned = coco_utils_pty::spawn_pty_process(
        bin.to_str().unwrap(),
        &[],
        config_dir.path(),
        &env,
        &None,
        TerminalSize::default(),
    )
    .await
    .expect("spawn coco teammate in PTY");

    // Drain the PTY output in the background so the child never blocks on a
    // full stdout channel (we assert on the mock request, not on TUI output).
    let mut stdout_rx = spawned.stdout_rx;
    let drain = tokio::spawn(async move { while stdout_rx.recv().await.is_some() {} });

    // 5. Poll the mock for a `POST /messages` carrying the framed prompt. The
    //    pump polls at 500 ms; the child does a full session bootstrap, so
    //    budget generously and poll (no fixed sleep-then-assert).
    let deadline = Instant::now() + Duration::from_secs(90);
    let mut saw_prompt = false;
    while Instant::now() < deadline {
        if let Some(reqs) = server.received_requests().await
            && reqs
                .iter()
                .any(|r| String::from_utf8_lossy(&r.body).contains(PROMPT_MARKER))
        {
            saw_prompt = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // 6. Teardown: kill the child (terminate the process group) + stop the drain.
    spawned.session.terminate();
    drain.abort();

    assert!(
        saw_prompt,
        "the teammate process never issued an LLM turn carrying the seeded \
         prompt within the deadline — boot/pump/turn path is broken"
    );
}
