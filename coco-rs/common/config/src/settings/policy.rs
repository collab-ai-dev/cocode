//! Enterprise/MDM policy settings loading.
//!
//! "First source wins" across policy tiers. File-based policy merges
//! `managed-settings.json` first, then sorted `managed-settings.d/*.json`
//! drop-ins.
//! Sources in order: remote > MDM/plist/HKLM > file > HKCU.

use super::Settings;
use crate::global_config;
use std::path::Path;
use std::path::PathBuf;

/// Load policy settings from enterprise/managed sources.
/// Returns the first non-empty source found.
///
/// Sources checked in order:
/// 1. Remote managed (cached from API sync)
/// 2. OS-level MDM (macOS plist / Windows HKLM)
/// 3. File-based managed-settings.json + .d/
pub fn load_policy_settings() -> Option<Settings> {
    let managed_path = global_config::managed_settings_path();
    let mut merged = serde_json::Value::Object(serde_json::Map::new());
    let mut loaded = false;

    if managed_path.exists()
        && let Ok(content) = std::fs::read_to_string(&managed_path)
        && let Ok(value) = crate::jsonc::parse_value(&content)
        && crate::settings::reject_unsupported_settings_keys(&value).is_ok()
    {
        merge_policy_value(&mut merged, &value);
        loaded = true;
    }

    for path in managed_dropin_paths(&managed_path).unwrap_or_default() {
        if let Ok(content) = std::fs::read_to_string(path)
            && let Ok(value) = crate::jsonc::parse_value(&content)
            && crate::settings::reject_unsupported_settings_keys(&value).is_ok()
        {
            merge_policy_value(&mut merged, &value);
            loaded = true;
        }
    }

    if loaded {
        serde_json::from_value::<Settings>(merged).ok()
    } else {
        None
    }
}

pub(super) fn managed_dropin_paths(managed_path: &Path) -> crate::Result<Vec<PathBuf>> {
    let managed_dir = managed_path.with_extension("d");
    if !managed_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut paths = Vec::new();
    for entry in std::fs::read_dir(&managed_dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

fn merge_policy_value(base: &mut serde_json::Value, overlay: &serde_json::Value) {
    let preserve_force_remote_refresh =
        force_remote_settings_refresh(base) || force_remote_settings_refresh(overlay);
    crate::settings::merge::deep_merge(base, overlay);
    if preserve_force_remote_refresh && let Some(obj) = base.as_object_mut() {
        obj.insert(
            "forceRemoteSettingsRefresh".to_string(),
            serde_json::Value::Bool(true),
        );
    }
}

fn force_remote_settings_refresh(value: &serde_json::Value) -> bool {
    value
        .pointer("/forceRemoteSettingsRefresh")
        .or_else(|| value.pointer("/force_remote_settings_refresh"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}
