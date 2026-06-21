//! Enterprise/MDM policy settings loading.
//!
//! "First source wins" — highest-priority source provides ALL policy settings.
//! Sources in order: remote > MDM/plist/HKLM > file > HKCU.

use super::Settings;
use crate::global_config;

/// Load policy settings from enterprise/managed sources.
/// Returns the first non-empty source found.
///
/// Sources checked in order:
/// 1. Remote managed (cached from API sync)
/// 2. OS-level MDM (macOS plist / Windows HKLM)
/// 3. File-based managed-settings.json + .d/
pub fn load_policy_settings() -> Option<Settings> {
    // 1. Try file-based managed-settings.json
    let managed_path = global_config::managed_settings_path();
    if managed_path.exists()
        && let Ok(content) = std::fs::read_to_string(&managed_path)
        && let Ok(settings) = crate::settings::parse_settings(&content)
    {
        return Some(settings);
    }

    // 2. Try managed-settings.d/*.json (sorted drop-in fragments)
    let managed_dir = managed_path.with_extension("d");
    if managed_dir.is_dir()
        && let Ok(entries) = std::fs::read_dir(&managed_dir)
    {
        let mut fragments: Vec<_> = entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
            .collect();
        fragments.sort_by_key(std::fs::DirEntry::path);

        let mut merged = serde_json::Value::Object(serde_json::Map::new());
        for entry in fragments {
            if let Ok(content) = std::fs::read_to_string(entry.path())
                && let Ok(value) = crate::jsonc::parse_value(&content)
            {
                crate::settings::merge::deep_merge(&mut merged, &value);
            }
        }

        if crate::settings::reject_unsupported_settings_keys(&merged).is_err() {
            return None;
        }
        if let Ok(settings) = serde_json::from_value::<Settings>(merged) {
            return Some(settings);
        }
    }

    None
}
