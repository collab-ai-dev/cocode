use super::*;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

#[test]
fn score_for_zero_count_is_zero() {
    let s = SkillUsageStats {
        usage_count: 0,
        last_used_at_ms: 1_000_000_000,
    };
    assert_eq!(score_for_at(&s, 2_000_000_000), 0.0);
}

#[test]
fn score_for_fresh_use_full_weight() {
    // Just used (0 days ago) — recency = 1.0, score = usage_count.
    let s = SkillUsageStats {
        usage_count: 4,
        last_used_at_ms: 1_000_000_000,
    };
    assert_eq!(score_for_at(&s, 1_000_000_000), 4.0);
}

#[test]
fn score_decays_seven_days_halves() {
    // 7-day half-life — at exactly 7 days the score is half.
    let day_ms = 1000 * 60 * 60 * 24;
    let s = SkillUsageStats {
        usage_count: 10,
        last_used_at_ms: 0,
    };
    let score = score_for_at(&s, 7 * day_ms);
    // Allow tiny floating-point slack.
    assert!((score - 5.0).abs() < 1e-9, "expected ~5.0, got {score}");
}

#[test]
fn score_clamped_to_min_recency_factor() {
    // 365 days out the raw factor is far below 0.1 — we clamp.
    let day_ms = 1000 * 60 * 60 * 24;
    let s = SkillUsageStats {
        usage_count: 3,
        last_used_at_ms: 0,
    };
    let score = score_for_at(&s, 365 * day_ms);
    // 3 * 0.1 = 0.3 exactly.
    assert!((score - 0.3).abs() < 1e-9, "expected 0.3, got {score}");
}

#[test]
fn record_then_load_roundtrip() {
    let tmp = TempDir::new().unwrap();
    record(tmp.path(), "test-skill");
    let map = load_all(tmp.path());
    let stats = map.get("test-skill").expect("entry persisted");
    assert_eq!(stats.usage_count, 1);
    assert!(stats.last_used_at_ms > 0);
}

#[test]
fn record_debounce_skips_duplicate_within_window() {
    // The 60-second debounce is process-local — without it we'd
    // pound the disk on rapid invocations. Two records within the
    // window should yield usage_count = 1, not 2.
    reset_debounce_for_tests();
    let tmp = TempDir::new().unwrap();
    let name = "debounce-roundtrip-skill";
    record(tmp.path(), name);
    record(tmp.path(), name);
    let map = load_all(tmp.path());
    assert_eq!(
        map.get(name).map(|s| s.usage_count),
        Some(1),
        "second record within debounce window must be a no-op"
    );
}

#[test]
fn record_failure_keeps_debounce_open_for_retry() {
    // Write to a non-existent parent that we deliberately prevent
    // create_dir_all from succeeding: use a path under a regular
    // file. The first record must fail, and a second record on the
    // same skill in the same window must NOT be no-op'd by debounce
    // — the retry needs a clean slate.
    reset_debounce_for_tests();
    let tmp = TempDir::new().unwrap();
    let blocker = tmp.path().join("not-a-dir");
    std::fs::write(&blocker, "hi").unwrap();
    let bad_home = blocker.join("nested");
    let name = "retry-skill";
    record(&bad_home, name); // fails: create_dir_all on a file path
    // Sanity: nothing landed.
    assert!(load_all(&bad_home).is_empty());
    // Second call to a NEW good path must not be blocked.
    let good_home = tmp.path().join("good");
    record(&good_home, name);
    let map = load_all(&good_home);
    assert_eq!(
        map.get(name).map(|s| s.usage_count),
        Some(1),
        "post-failure retry must succeed in same debounce window"
    );
}

#[test]
fn record_ignores_empty_name() {
    let tmp = TempDir::new().unwrap();
    // debug_assert in record fires under debug builds; we wrap to
    // catch it. The behavior under release is "silently skip" which
    // is the contract we want to verify.
    std::panic::set_hook(Box::new(|_| {}));
    let _ = std::panic::catch_unwind(|| record(tmp.path(), ""));
    let _ = std::panic::take_hook();
    let map = load_all(tmp.path());
    assert!(map.is_empty(), "empty name must not produce an entry");
}

#[test]
fn write_is_atomic_no_partial_files() {
    // After a record, the only files in config_home are the JSON
    // itself — tempfile drops/persists must clean up properly.
    reset_debounce_for_tests();
    let tmp = TempDir::new().unwrap();
    record(tmp.path(), "atomic-test");
    let entries: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        entries,
        vec!["skill_usage.json"],
        "atomic-rename write must leave no stray .tmp siblings"
    );
}

#[test]
fn load_all_returns_empty_on_missing_file() {
    let tmp = TempDir::new().unwrap();
    let map = load_all(tmp.path());
    assert!(map.is_empty());
}

#[test]
fn load_all_returns_empty_on_corrupt_file() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("skill_usage.json"), "not valid json {").unwrap();
    let map = load_all(tmp.path());
    assert!(map.is_empty(), "corrupt file must not break the popup");
}

#[test]
fn parses_camel_case_aliases() {
    // A skill_usage.json with camelCase keys is read via serde alias
    // so migrating users don't lose their history.
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("skill_usage.json"),
        r#"{ "skills": { "legacy-skill": { "usageCount": 9, "lastUsedAt": 12345 } } }"#,
    )
    .unwrap();
    let map = load_all(tmp.path());
    let stats = map.get("legacy-skill").expect("camelCase entry parsed");
    assert_eq!(stats.usage_count, 9);
    assert_eq!(stats.last_used_at_ms, 12345);
}
