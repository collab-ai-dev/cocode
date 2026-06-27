use super::*;

#[test]
fn backoff_grows_then_caps_at_one_week() {
    assert_eq!(backoff_secs(0), BASE_BACKOFF_SECS);
    assert_eq!(backoff_secs(1), BASE_BACKOFF_SECS * 2);
    assert_eq!(backoff_secs(2), BASE_BACKOFF_SECS * 4);
    // Caps at one week and never overflows for large attempt counts.
    assert_eq!(backoff_secs(30), MAX_BACKOFF_SECS);
    assert_eq!(backoff_secs(u32::MAX), MAX_BACKOFF_SECS);
}

#[test]
fn state_roundtrips_through_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("plugins").join(STATE_FILE);
    let state = AutoInstallState {
        installed: false,
        attempts: 3,
        last_attempt: None,
    };
    save_state(&path, &state).unwrap();
    let loaded = load_state(&path);
    assert_eq!(loaded.attempts, 3);
    assert!(!loaded.installed);
}

#[test]
fn load_state_missing_file_is_default() {
    let tmp = tempfile::tempdir().unwrap();
    let s = load_state(&tmp.path().join("nope.json"));
    assert_eq!(s.attempts, 0);
    assert!(!s.installed);
    assert!(s.last_attempt.is_none());
}

#[tokio::test]
async fn already_registered_short_circuits_without_fetch() {
    let tmp = tempfile::tempdir().unwrap();
    let plugins_dir = tmp.path().join("plugins");
    // Pre-register the official marketplace (its github anthropics source
    // passes the reserved-name validation).
    let mut mgr = MarketplaceManager::new(plugins_dir.clone());
    mgr.register_marketplace(
        OFFICIAL_MARKETPLACE_NAME,
        official_marketplace_source(),
        &plugins_dir
            .join("marketplaces")
            .join(OFFICIAL_MARKETPLACE_NAME)
            .to_string_lossy(),
    )
    .unwrap();

    let outcome = ensure_official_marketplace(plugins_dir).await;
    assert_eq!(outcome, OfficialInstallOutcome::AlreadyInstalled);
}

#[tokio::test]
async fn recent_failure_backs_off_without_fetch() {
    let tmp = tempfile::tempdir().unwrap();
    let plugins_dir = tmp.path().join("plugins");
    // One failed attempt just now → still inside the backoff window, so the
    // call must return Backoff before reaching the (network) fetch.
    save_state(
        &plugins_dir.join(STATE_FILE),
        &AutoInstallState {
            installed: false,
            attempts: 1,
            last_attempt: Some(Utc::now()),
        },
    )
    .unwrap();

    let outcome = ensure_official_marketplace(plugins_dir).await;
    assert_eq!(outcome, OfficialInstallOutcome::Backoff);
}

#[tokio::test]
async fn max_attempts_exhausts_without_fetch() {
    let tmp = tempfile::tempdir().unwrap();
    let plugins_dir = tmp.path().join("plugins");
    save_state(
        &plugins_dir.join(STATE_FILE),
        &AutoInstallState {
            installed: false,
            attempts: MAX_ATTEMPTS,
            last_attempt: None,
        },
    )
    .unwrap();

    let outcome = ensure_official_marketplace(plugins_dir).await;
    assert_eq!(outcome, OfficialInstallOutcome::Exhausted);
}
