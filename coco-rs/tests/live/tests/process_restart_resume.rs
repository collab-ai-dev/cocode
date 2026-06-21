//! Process-restart resume e2e — proves a durable rollout survives a real
//! `coco` process restart and the resumed history reaches the model.
//!
//! Spawns the REAL `coco` binary twice in headless (`--print`) mode,
//! sharing one `COCO_CONFIG_DIR` and a fixed `--session-id`:
//!   run 1: `coco --print --session-id S "<ALPHA>"`  → persists the rollout
//!   (process exits = the "kill")
//!   run 2: `coco --print --resume S "<BETA>"`        → loads the rollout
//! Both point at a wiremock SSE server. We then assert that some request
//! captured during run 2 carries BOTH the run-1 prompt (replayed history)
//! and the run-2 prompt — i.e. the persisted transcript crossed the
//! process boundary and was sent to the model. This is coco's analogue of
//! codex's app-server `thread_resume` (kill + respawn on the same home).
//!
//! Network-free (wiremock) and build-gated: skips cleanly when the `coco`
//! binary isn't built, so it never flakes a CI run that didn't build it.

use std::path::Path;
use std::process::Output;
use std::time::Duration;

use anyhow::Result;
use anyhow::anyhow;
use serde_json::json;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

const ALPHA: &str = "ALPHA-PROMPT-marker";
const BETA: &str = "BETA-PROMPT-marker";
const MODEL: &str = "anthropic/claude-opus-4-7";

/// A minimal Anthropic Messages end_turn text SSE — enough for the
/// headless turn to complete and persist without needing tools.
fn text_sse(text: &str) -> String {
    [
        ("message_start", json!({"type":"message_start","message":{"id":"msg","model":"claude-test","usage":{"input_tokens":10},"content":[]}})),
        ("content_block_start", json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})),
        ("content_block_delta", json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}})),
        ("content_block_stop", json!({"type":"content_block_stop","index":0})),
        ("message_delta", json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}})),
        ("message_stop", json!({"type":"message_stop"})),
    ]
    .iter()
    .map(|(e, d)| format!("event: {e}\ndata: {}\n\n", serde_json::to_string(d).unwrap()))
    .collect()
}

async fn run_coco(
    bin: &Path,
    config_dir: &Path,
    cwd: &Path,
    prompt: &str,
    session_args: &[&str],
) -> Result<Output> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.current_dir(cwd)
        .env("COCO_CONFIG_DIR", config_dir)
        .env("ANTHROPIC_API_KEY", "test-key")
        .arg("--non-interactive")
        .arg("--output-format")
        .arg("text")
        .arg("--models.main")
        .arg(MODEL)
        // `prompt` is a `-p`/`--prompt` flag, not a positional (a bare arg
        // is parsed as a subcommand).
        .arg("--prompt")
        .arg(prompt)
        .args(session_args)
        .stdin(std::process::Stdio::null());
    let out = tokio::time::timeout(Duration::from_secs(90), cmd.output())
        .await
        .map_err(|_| anyhow!("coco subprocess timed out"))??;
    Ok(out)
}

#[tokio::test]
async fn process_restart_resume_carries_history_to_model() -> Result<()> {
    let Ok(bin) = coco_utils_cargo_bin::cargo_bin("coco") else {
        eprintln!(
            "[process_restart_resume] `coco` binary not built — skipping. \
             Build it (e.g. `cargo build -p coco-cli --bin coco`) to run this test."
        );
        return Ok(());
    };

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(text_sse("acknowledged"), "text/event-stream"),
        )
        .mount(&server)
        .await;

    let config_dir = tempfile::tempdir()?;
    let cwd = tempfile::tempdir()?;
    // Point the builtin anthropic provider at the mock via a providers.json
    // overlay under COCO_CONFIG_DIR (the binary reads base_url from config,
    // not from ANTHROPIC_BASE_URL).
    std::fs::write(
        config_dir.path().join("providers.json"),
        json!({ "anthropic": { "base_url": server.uri(), "api_key": "test-key" } }).to_string(),
    )?;

    // Run 1 — a fresh session; the per-turn finalize persists the rollout
    // under `<config_home>/projects/<cwd-slug>/`, then the process exits.
    let out1 = run_coco(&bin, config_dir.path(), cwd.path(), ALPHA, &[]).await?;
    assert!(
        out1.status.success(),
        "run 1 failed: status={:?}\nstdout={}\nstderr={}",
        out1.status,
        String::from_utf8_lossy(&out1.stdout),
        String::from_utf8_lossy(&out1.stderr),
    );

    // Run 2 — a brand-new process resumes the most-recent session (the one
    // run 1 just persisted; same cwd ⇒ same project slug) and adds a turn.
    // `--continue` avoids needing to know the generated session id.
    let out2 = run_coco(&bin, config_dir.path(), cwd.path(), BETA, &["--continue"]).await?;
    assert!(
        out2.status.success(),
        "run 2 (resume) failed: status={:?}\nstdout={}\nstderr={}",
        out2.status,
        String::from_utf8_lossy(&out2.stdout),
        String::from_utf8_lossy(&out2.stderr),
    );

    let requests = server
        .received_requests()
        .await
        .expect("wiremock should record requests");
    assert!(
        requests.len() >= 2,
        "expected ≥2 POST /messages across both runs, got {}",
        requests.len(),
    );

    // The durable rollout survived the restart iff some request (necessarily
    // from run 2) carries BOTH the run-1 prompt (replayed history) and the
    // run-2 prompt. Run-1 requests can only contain ALPHA.
    let carried_history = requests.iter().any(|r| {
        let body = String::from_utf8_lossy(&r.body);
        body.contains(ALPHA) && body.contains(BETA)
    });
    assert!(
        carried_history,
        "resumed request should carry run-1 history ({ALPHA}) alongside the run-2 prompt \
         ({BETA}) — the persisted transcript must cross the process boundary; \
         {} requests captured",
        requests.len(),
    );

    Ok(())
}
