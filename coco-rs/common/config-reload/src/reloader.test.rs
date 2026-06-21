use super::*;
use coco_config::CatalogPaths;
use coco_config::EnvSnapshot;
use std::time::Duration;
use tempfile::TempDir;

/// Spawn a reloader with TempDir-isolated catalog paths and a
/// minimally-populated `EnvSnapshot` carrying just enough state to
/// satisfy the runtime builder (a Main model selection — the
/// builder errors out without one). All other env keys stay
/// unset so the test is independent of host environment.
async fn spawn_isolated(home: &std::path::Path) -> RuntimeReloader {
    let opts = ReloadOptions::new(home)
        .with_catalog_paths(CatalogPaths::rooted(home))
        .with_env_factory(test_env_with_main_model)
        .with_debounce(Duration::from_millis(50));
    RuntimeReloader::spawn(opts).expect("spawn reloader")
}

/// Empty-except-Main-model env. The runtime builder requires a
/// Main model selection (TS parity: multi-provider SDK refuses
/// silent defaults — see `runtime.rs:412-428` `no Main model
/// configured` error). `anthropic/claude-sonnet-4-6` is a
/// builtin-roster slug so it resolves without needing a custom
/// `models.json`.
fn test_env_with_main_model() -> EnvSnapshot {
    EnvSnapshot::from_pairs(std::iter::once((
        coco_config::EnvKey::CocoModelMain,
        "anthropic/claude-sonnet-4-6",
    )))
}

fn empty_env() -> EnvSnapshot {
    EnvSnapshot::from_pairs(std::iter::empty::<(coco_config::EnvKey, &str)>())
}

#[tokio::test(flavor = "multi_thread")]
async fn initial_publish_succeeds_with_no_catalog_files() {
    let tmp = TempDir::new().unwrap();
    let reloader = spawn_isolated(tmp.path()).await;
    let snapshot = reloader.current();
    // Builtin providers always populated.
    assert!(!snapshot.providers.is_empty());
    assert!(snapshot.providers.contains_key("anthropic"));
}

#[tokio::test(flavor = "multi_thread")]
async fn fails_outside_tokio_runtime() {
    // Spawning a blocking thread, then running spawn() — `Handle::try_current`
    // should fail and propagate as `Err`, not panic.
    let result = std::thread::spawn(|| {
        let tmp = TempDir::new().unwrap();
        let opts = ReloadOptions::new(tmp.path())
            .with_catalog_paths(CatalogPaths::rooted(tmp.path()))
            .with_env_factory(empty_env);
        RuntimeReloader::spawn(opts)
    })
    .join()
    .expect("test thread join");

    assert!(result.is_err(), "must return Err outside Tokio runtime");
    let msg = match result {
        Err(e) => e.to_string(),
        Ok(_) => unreachable!(),
    };
    assert!(
        msg.contains("Tokio"),
        "expected Tokio precondition error, got: {msg}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn drop_aborts_spawned_task() {
    let tmp = TempDir::new().unwrap();
    let mut reloader = spawn_isolated(tmp.path()).await;
    let publisher = reloader.publisher();
    let handle = reloader.steal_join_handle_for_test();
    drop(reloader);

    // Yield long enough for `JoinHandle::abort` to drive the spawned
    // task to completion. `tokio::yield_now` alone isn't sufficient
    // because the watcher's `recv().await` may be parked; we wait on
    // the handle directly to confirm termination.
    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("reloader task must terminate within 2s of Drop")
        .ok(); // task may finish with `Err(JoinError::aborted)` — that's fine

    // After drop, current snapshot is still readable via the
    // outstanding publisher Arc (publisher is not the JoinHandle).
    let _snapshot = publisher.current();
}

#[tokio::test(flavor = "multi_thread")]
async fn config_file_appearance_triggers_rebuild() {
    let tmp = TempDir::new().unwrap();
    let reloader = spawn_isolated(tmp.path()).await;
    let mut rx = reloader.publisher().subscribe();

    // mark_changed twice (initial + post-write) — the watch::Receiver
    // surfaces the publish via the changed/borrow_and_update flow.
    let providers_path = tmp.path().join("providers.json");
    let json = r#"{
        // Hot reload reuses coco-config's JSONC parser.
        "internal-router": {
            "api": "openai_compat",
            "env_key": "INTERNAL_KEY",
            "base_url": "https://internal/v1",
        },
    }"#;
    std::fs::write(&providers_path, json).unwrap();

    // Wait for the rebuild publish (debounce + filesystem race).
    let result = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if rx.changed().await.is_err() {
                break None;
            }
            let snap = rx.borrow_and_update().clone();
            if snap.providers.contains_key("internal-router") {
                break Some(snap);
            }
        }
    })
    .await;

    let snapshot = result
        .expect("rebuild within timeout")
        .expect("rebuild produced a snapshot");
    let entry = snapshot.providers.get("internal-router").unwrap();
    assert_eq!(entry.base_url, "https://internal/v1");
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_catalog_keeps_prior_snapshot() {
    let tmp = TempDir::new().unwrap();
    let reloader = spawn_isolated(tmp.path()).await;
    let mut error_rx = reloader.subscribe_errors();
    let initial = reloader.current();
    let initial_keys: std::collections::BTreeSet<String> =
        initial.providers.keys().cloned().collect();

    // Write garbage to providers.json. Reloader logs an error and
    // retains the prior snapshot.
    let providers_path = tmp.path().join("providers.json");
    std::fs::write(&providers_path, "not valid json {{{").unwrap();

    let reload_error = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match error_rx.recv().await {
                Ok(err) if err.path == providers_path => break err,
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("reload error channel closed")
                }
            }
        }
    })
    .await
    .expect("reload error within timeout");
    assert_eq!(
        reload_error.kind,
        TrackedKind::Settings(coco_config::WatchedKind::ProvidersCatalog)
    );
    assert!(
        reload_error.message.contains("failed to parse"),
        "unexpected reload error: {}",
        reload_error.message
    );

    let after = reloader.current();
    let after_keys: std::collections::BTreeSet<String> = after.providers.keys().cloned().collect();

    // Stronger than count: full key-set equality and Arc identity.
    // Either holds independently of the other:
    //   - ptr_eq:    no publish happened (no rebuild attempt or
    //                publish skipped because rebuild errored).
    //   - key set:   if a publish DID happen, it must have produced
    //                the same provider set (which would only be the
    //                case if parsing succeeded against the garbage,
    //                which is impossible).
    assert!(
        std::sync::Arc::ptr_eq(&initial, &after) || initial_keys == after_keys,
        "malformed catalog rebuilt to a different provider set: initial={initial_keys:?}, after={after_keys:?}"
    );
    assert_eq!(
        initial_keys, after_keys,
        "provider key set must be unchanged"
    );
}
