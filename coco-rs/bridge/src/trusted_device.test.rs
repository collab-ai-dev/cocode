use tempfile::TempDir;

use super::TrustedDeviceStore;

#[test]
fn trust_and_lookup_round_trip() {
    let mut store = TrustedDeviceStore::new();
    assert!(!store.is_trusted("dev-1"));
    store.trust("dev-1", "VS Code on mac");
    assert!(store.is_trusted("dev-1"));

    let entries = store.sorted_by_recency();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].label, "VS Code on mac");
}

#[test]
fn re_trust_updates_last_seen_not_added_at() {
    let mut store = TrustedDeviceStore::new();
    store.trust("dev-1", "initial");
    let initial_added = store.devices["dev-1"].added_at;

    // Wait a tiny bit to ensure last_seen can tick (unix seconds
    // granularity may collapse, but the invariant we check is that
    // added_at is preserved regardless).
    store.trust("dev-1", "rename-ignored");
    let after_added = store.devices["dev-1"].added_at;
    assert_eq!(initial_added, after_added);
}

#[test]
fn revoke_removes_device() {
    let mut store = TrustedDeviceStore::new();
    store.trust("dev-1", "x");
    assert!(store.revoke("dev-1"));
    assert!(!store.is_trusted("dev-1"));
    assert!(!store.revoke("dev-1"));
}

#[test]
fn load_missing_file_returns_empty() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("nope.json");
    let store = TrustedDeviceStore::load_from(&path);
    assert!(store.devices.is_empty());
}

#[test]
fn save_and_load_round_trip() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("subdir").join("trusted.json");

    let mut store = TrustedDeviceStore::new();
    store.trust("a", "A");
    store.trust("b", "B");
    store.save_to(&path).unwrap();

    let loaded = TrustedDeviceStore::load_from(&path);
    assert_eq!(loaded.devices.len(), 2);
    assert!(loaded.is_trusted("a"));
    assert!(loaded.is_trusted("b"));
}

#[test]
fn load_corrupted_file_returns_empty() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("bad.json");
    std::fs::write(&path, b"not-json").unwrap();
    let store = TrustedDeviceStore::load_from(&path);
    assert!(store.devices.is_empty());
}
