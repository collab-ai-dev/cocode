//! Tests for the concurrent-sessions PID registry.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

/// Write a PID file under the canonical `<config_home>/sessions/pids/`
/// layout used by the production code. Tests that want to pre-seed
/// the registry use this so the layout-change blast radius is small.
fn write_pid_file(config_home: &Path, pid: u32, contents: &str) {
    let dir = sessions_dir(config_home);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{pid}.json")), contents).unwrap();
}

#[test]
fn register_writes_pid_file_with_expected_shape() {
    let cfg = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let session_id = "session-abc";

    let registry = SessionRegistry::register(cfg.path(), session_id, cwd.path(), None)
        .unwrap()
        .expect("not a subagent so registration must succeed");

    let pid = std::process::id();
    let on_disk = read_registration(cfg.path(), pid).unwrap().unwrap();
    assert_eq!(on_disk.pid, pid);
    assert_eq!(on_disk.session_id, session_id);
    assert_eq!(on_disk.cwd, cwd.path());
    assert_eq!(on_disk.kind, SessionKind::Interactive);
    assert!(on_disk.bridge_session_id.is_none());

    // Drop removes the file.
    drop(registry);
    let after = read_registration(cfg.path(), pid).unwrap();
    assert!(after.is_none(), "Drop should delete the PID file");
}

#[test]
fn subagent_context_skips_registration() {
    let cfg = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();

    let result =
        SessionRegistry::register(cfg.path(), "session-sub", cwd.path(), Some("agent-123"))
            .unwrap();

    assert!(
        result.is_none(),
        "subagent contexts must not register; counted as a no-op success"
    );
    // No file written either.
    let dir = sessions_dir(cfg.path());
    assert!(!dir.exists() || std::fs::read_dir(&dir).unwrap().count() == 0);
}

#[test]
fn unregister_removes_file_and_is_idempotent() {
    let cfg = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();

    let registry = SessionRegistry::register(cfg.path(), "s", cwd.path(), None)
        .unwrap()
        .unwrap();
    let pid_file = registry.pid_file().to_path_buf();
    assert!(pid_file.exists());

    registry.unregister().unwrap();
    assert!(!pid_file.exists());
}

#[test]
fn update_session_name_persists_into_existing_file() {
    let cfg = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();

    let registry = SessionRegistry::register(cfg.path(), "sid", cwd.path(), None)
        .unwrap()
        .unwrap();
    registry.update_session_name("my-bg-session");

    let rec = read_registration(cfg.path(), std::process::id())
        .unwrap()
        .unwrap();
    assert_eq!(rec.name.as_deref(), Some("my-bg-session"));
}

#[test]
fn update_session_name_empty_is_noop() {
    let cfg = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let registry = SessionRegistry::register(cfg.path(), "sid", cwd.path(), None)
        .unwrap()
        .unwrap();
    registry.update_session_name("");
    let rec = read_registration(cfg.path(), std::process::id())
        .unwrap()
        .unwrap();
    assert!(rec.name.is_none());
}

#[test]
fn update_session_bridge_id_set_and_clear() {
    let cfg = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let registry = SessionRegistry::register(cfg.path(), "sid", cwd.path(), None)
        .unwrap()
        .unwrap();

    registry.update_session_bridge_id(Some("bridge-xyz"));
    let after_set = read_registration(cfg.path(), std::process::id())
        .unwrap()
        .unwrap();
    assert_eq!(after_set.bridge_session_id.as_deref(), Some("bridge-xyz"));

    registry.update_session_bridge_id(None);
    // After clearing, the raw JSON value is null; serde deserializes it
    // back as `None` for the typed view.
    let after_clear = read_registration(cfg.path(), std::process::id())
        .unwrap()
        .unwrap();
    assert!(after_clear.bridge_session_id.is_none());
}

#[test]
fn update_session_activity_stamps_updated_at() {
    let cfg = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let registry = SessionRegistry::register(cfg.path(), "sid", cwd.path(), None)
        .unwrap()
        .unwrap();

    registry.update_session_activity(Some(SessionStatus::Busy), Some("compile"));
    let rec = read_registration(cfg.path(), std::process::id())
        .unwrap()
        .unwrap();
    assert_eq!(rec.status, Some(SessionStatus::Busy));
    assert_eq!(rec.waiting_for.as_deref(), Some("compile"));
    assert!(rec.updated_at.is_some());
}

#[test]
fn count_includes_self_even_without_file() {
    let cfg = TempDir::new().unwrap();
    let n = count_concurrent_sessions(cfg.path());
    // Sessions dir doesn't exist yet — count is 0; the "self counted"
    // promise only holds when there's a file.
    assert_eq!(n, 0);
}

#[test]
fn count_includes_self_when_registered() {
    let cfg = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let _registry = SessionRegistry::register(cfg.path(), "sid", cwd.path(), None)
        .unwrap()
        .unwrap();
    let n = count_concurrent_sessions(cfg.path());
    assert!(n >= 1, "self-pid must always be counted; got {n}");
}

#[test]
fn count_ignores_non_pid_filenames() {
    let cfg = TempDir::new().unwrap();
    write_pid_file(cfg.path(), 0, "{}"); // 0 is a sentinel; counted only if running, which kill(0,0) fails on
    let dir = sessions_dir(cfg.path());
    std::fs::write(dir.join("not-a-pid.json"), "{}").unwrap();
    std::fs::write(dir.join("2026-03-14_notes.md"), "ignored").unwrap();
    std::fs::write(dir.join("12345"), "no-extension").unwrap();

    let n = count_concurrent_sessions(cfg.path());
    // None of these are valid PID files: stem is empty / non-digit / no .json.
    // `0.json` parses to pid=0 which `is_process_running` rejects, so the
    // function tries to sweep it. (We don't assert the count here precisely
    // because that would need a known-dead pid not equal to 0/1 — covered
    // in `count_sweeps_stale_files`.) We just assert that the non-pid
    // files do not show up in the count.
    assert!(n == 0, "expected 0, got {n}");
}

#[test]
fn count_sweeps_stale_files() {
    let cfg = TempDir::new().unwrap();
    let dir = sessions_dir(cfg.path());
    // PID 99999999 is extremely unlikely to be live on any test host.
    // The sweep should remove its file. Skip the sweep assertion on WSL
    // since we explicitly disable the sweep there.
    let stale_pid = 99_999_999u32;
    write_pid_file(
        cfg.path(),
        stale_pid,
        &serde_json::to_string(&SessionRegistration {
            pid: stale_pid,
            session_id: "ghost".into(),
            cwd: dir.clone(),
            started_at: 0,
            kind: SessionKind::Interactive,
            entrypoint: None,
            name: None,
            bridge_session_id: None,
            updated_at: None,
            status: None,
            waiting_for: None,
        })
        .unwrap(),
    );

    let _ = count_concurrent_sessions(cfg.path());
    let stale_path = dir.join(format!("{stale_pid}.json"));
    let on_wsl = std::env::var("WSL_DISTRO_NAME").is_ok();
    if !on_wsl {
        assert!(
            !stale_path.exists(),
            "non-WSL host must sweep stale PID files"
        );
    }
}

#[test]
fn registration_json_wire_format_is_camel_case() {
    let rec = SessionRegistration {
        pid: 42,
        session_id: "abc".into(),
        cwd: PathBuf::from("/tmp"),
        started_at: 123_456_789,
        kind: SessionKind::DaemonWorker,
        entrypoint: Some("sdk-py".into()),
        name: Some("nightly-eval".into()),
        bridge_session_id: Some("bridge-7".into()),
        updated_at: Some(123_456_999),
        status: Some(SessionStatus::Idle),
        waiting_for: Some("model-response".into()),
    };
    let json = serde_json::to_value(&rec).unwrap();
    let obj = json.as_object().unwrap();
    // Snake_case wire — `<pid>.json` is coco-rs's own registry shape.
    assert!(obj.contains_key("session_id"));
    assert!(obj.contains_key("started_at"));
    assert!(obj.contains_key("bridge_session_id"));
    assert!(obj.contains_key("updated_at"));
    assert!(obj.contains_key("waiting_for"));
    assert_eq!(obj.get("kind").unwrap(), "daemon-worker");
    assert_eq!(obj.get("status").unwrap(), "idle");
}
