use super::McpNeedsAuthCache;
use super::TTL_MS;
use pretty_assertions::assert_eq;

fn cache() -> (tempfile::TempDir, McpNeedsAuthCache) {
    let dir = tempfile::tempdir().expect("create temp dir");
    let cache = McpNeedsAuthCache::new(dir.path());
    (dir, cache)
}

#[tokio::test]
async fn test_set_then_cached_within_ttl() {
    let (_dir, cache) = cache();
    let now = 1_000_000;
    cache.set_at("srv", now).await;
    // Just inside the window.
    assert!(cache.is_cached_at("srv", now + TTL_MS - 1).await);
}

#[tokio::test]
async fn test_expired_after_ttl() {
    let (_dir, cache) = cache();
    let now = 1_000_000;
    cache.set_at("srv", now).await;
    // Exactly at the TTL boundary is treated as expired (strict `<`).
    assert!(!cache.is_cached_at("srv", now + TTL_MS).await);
    assert!(!cache.is_cached_at("srv", now + TTL_MS + 5_000).await);
}

#[tokio::test]
async fn test_absent_server_not_cached() {
    let (_dir, cache) = cache();
    assert!(!cache.is_cached_at("never-seen", 1_000_000).await);
}

#[tokio::test]
async fn test_clear_evicts_marker() {
    let (_dir, cache) = cache();
    let now = 2_000_000;
    cache.set_at("srv", now).await;
    assert!(cache.is_cached_at("srv", now).await);
    cache.clear("srv").await;
    assert!(!cache.is_cached_at("srv", now).await);
}

#[tokio::test]
async fn test_clear_is_per_server() {
    let (_dir, cache) = cache();
    let now = 2_000_000;
    cache.set_at("a", now).await;
    cache.set_at("b", now).await;
    cache.clear("a").await;
    assert!(!cache.is_cached_at("a", now).await);
    assert!(cache.is_cached_at("b", now).await);
}

/// A batched multi-server 401 storm must not corrupt the file: every concurrent
/// `set` survives (serialized via the shared write lock). Cloned caches share
/// the lock, mirroring cloned managers.
#[tokio::test]
async fn test_concurrent_set_storm_no_corruption() {
    let (_dir, cache) = cache();
    let now = 3_000_000;
    let mut handles = Vec::new();
    for i in 0..24 {
        let c = cache.clone();
        handles.push(tokio::spawn(async move {
            c.set_at(&format!("srv-{i}"), now).await;
        }));
    }
    for h in handles {
        h.await.expect("set task");
    }
    let mut present = 0;
    for i in 0..24 {
        if cache.is_cached_at(&format!("srv-{i}"), now).await {
            present += 1;
        }
    }
    assert_eq!(present, 24, "all concurrent set markers must survive");
}
