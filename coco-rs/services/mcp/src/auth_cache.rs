//! On-disk TTL cache marking MCP servers that recently required auth, so
//! repeated connect-401 + OAuth-discovery round-trips are suppressed within a
//! short window. The skip is what keeps print/headless startup fast when many
//! remote servers are configured but unauthenticated (each probe is a network
//! round-trip the batch would otherwise await every launch).
//!
//! Stored in `mcp-needs-auth-cache.json`. Keyed by the raw server name with a
//! millisecond timestamp; writes are serialized so a batched multi-server 401
//! storm can't corrupt the file.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use serde::Deserialize;
use serde::Serialize;
use tokio::sync::Mutex;

/// 15-minute TTL window.
const TTL_MS: i64 = 15 * 60 * 1000;

const CACHE_FILE: &str = "mcp-needs-auth-cache.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct NeedsAuthEntry {
    timestamp_ms: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct CacheFile {
    servers: HashMap<String, NeedsAuthEntry>,
}

/// Instance-owned needs-auth marker cache (no global state — the manager owns
/// it and clones share the write lock).
#[derive(Clone)]
pub struct McpNeedsAuthCache {
    path: PathBuf,
    /// Serializes read-modify-write so concurrent `set`s from a batched 401
    /// storm don't clobber each other. Shared across cloned managers.
    write_lock: Arc<Mutex<()>>,
}

impl McpNeedsAuthCache {
    pub fn new(config_home: &Path) -> Self {
        Self {
            path: config_home.join(CACHE_FILE),
            write_lock: Arc::new(Mutex::new(())),
        }
    }

    fn now_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    async fn read(&self) -> CacheFile {
        match tokio::fs::read_to_string(&self.path).await {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
            Err(_) => CacheFile::default(),
        }
    }

    async fn write(&self, file: &CacheFile) {
        if let Some(parent) = self.path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        if let Ok(serialized) = serde_json::to_string(file) {
            let _ = tokio::fs::write(&self.path, serialized).await;
        }
    }

    /// Whether `server` has a needs-auth marker still within the TTL window.
    pub async fn is_cached(&self, server: &str) -> bool {
        self.is_cached_at(server, Self::now_ms()).await
    }

    async fn is_cached_at(&self, server: &str, now_ms: i64) -> bool {
        // Read under the write lock so a concurrent `set` is fully visible.
        let _guard = self.write_lock.lock().await;
        match self.read().await.servers.get(server) {
            Some(entry) => now_ms - entry.timestamp_ms < TTL_MS,
            None => false,
        }
    }

    /// Mark `server` as needing auth, stamped now. Serialized RMW.
    pub async fn set(&self, server: &str) {
        self.set_at(server, Self::now_ms()).await;
    }

    async fn set_at(&self, server: &str, now_ms: i64) {
        let _guard = self.write_lock.lock().await;
        let mut file = self.read().await;
        file.servers.insert(
            server.to_string(),
            NeedsAuthEntry {
                timestamp_ms: now_ms,
            },
        );
        self.write(&file).await;
    }

    /// Evict `server`'s marker — after a successful (re)connect, or before a
    /// post-OAuth reconnect so the attempt isn't itself skipped. Per-server
    /// surgical eviction rather than wiping the whole file.
    pub async fn clear(&self, server: &str) {
        let _guard = self.write_lock.lock().await;
        let mut file = self.read().await;
        if file.servers.remove(server).is_some() {
            self.write(&file).await;
        }
    }
}

#[cfg(test)]
#[path = "auth_cache.test.rs"]
mod tests;
