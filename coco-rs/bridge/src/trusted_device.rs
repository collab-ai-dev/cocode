//! Trusted-device store for the IDE bridge.
//!
//! When an IDE connects for the first time the user approves it once;
//! the device's public fingerprint (usually a random UUID from the IDE
//! side) is recorded so subsequent connections skip the approval prompt.
//!
//! Storage format: a JSON file at `~/.coco/trusted-devices.json`
//! holding a list of entries `{ device_id, label, added_at, last_seen }`.
//! This module owns only the data model + serialization; the actual
//! filesystem I/O is performed by callers via `load_from` / `save_to`.

use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use serde::Deserialize;
use serde::Serialize;

/// A single trusted device record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrustedDevice {
    /// Opaque IDE-supplied identifier (persists across IDE restarts).
    pub device_id: String,
    /// Human-readable name shown in the UI (`"VS Code on macbook"`).
    pub label: String,
    /// First-trusted timestamp (seconds since epoch).
    pub added_at: i64,
    /// Most recent successful connection (seconds since epoch).
    pub last_seen: i64,
}

/// On-disk file shape. A newtype over a map rather than a bare `Vec` so
/// lookups are O(1) and the file survives a renamed device cleanly.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TrustedDeviceStore {
    #[serde(default)]
    pub devices: HashMap<String, TrustedDevice>,
}

impl TrustedDeviceStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether `device_id` is already trusted.
    pub fn is_trusted(&self, device_id: &str) -> bool {
        self.devices.contains_key(device_id)
    }

    /// Add or re-affirm a device, updating `last_seen` to now.
    pub fn trust(&mut self, device_id: impl Into<String>, label: impl Into<String>) {
        let id = device_id.into();
        let now = now_unix();
        self.devices
            .entry(id.clone())
            .and_modify(|d| d.last_seen = now)
            .or_insert_with(|| TrustedDevice {
                device_id: id,
                label: label.into(),
                added_at: now,
                last_seen: now,
            });
    }

    /// Update `last_seen` for an already-trusted device. No-op if the
    /// device isn't in the store (caller should call `trust()` instead).
    pub fn record_seen(&mut self, device_id: &str) {
        if let Some(dev) = self.devices.get_mut(device_id) {
            dev.last_seen = now_unix();
        }
    }

    /// Remove a device. Returns `true` when something was removed.
    pub fn revoke(&mut self, device_id: &str) -> bool {
        self.devices.remove(device_id).is_some()
    }

    /// Iterate entries sorted by `last_seen` desc for UI display.
    pub fn sorted_by_recency(&self) -> Vec<&TrustedDevice> {
        let mut v: Vec<&TrustedDevice> = self.devices.values().collect();
        v.sort_by_key(|d| -d.last_seen);
        v
    }

    /// Load from disk. Returns an empty store if the file is missing or
    /// unparseable (corrupted file → start fresh, don't panic the TUI).
    pub fn load_from(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Persist to disk atomically (write to `{path}.tmp`, then rename).
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("tmp");
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "trusted_device.test.rs"]
mod tests;
