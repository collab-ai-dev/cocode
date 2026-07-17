//! `coco config <action>` — get/set/list/reset for user settings.

use std::path::Path;

use anyhow::Result;
use coco_cli::ConfigAction;
use coco_config::global_config;

/// Interpret a CLI-supplied setting value as JSON, falling back to a string.
///
/// The shell hands us text, but settings are typed: `disable_bypass_mode true`
/// has to land as a bool, not `"true"`, or it silently fails to do anything.
/// JSON-first gets bools, numbers, arrays, and objects right, and the fallback
/// covers the common case of a bare string like a model id, which is not valid
/// JSON on its own. To force a string that *looks* like JSON, quote it:
/// `coco config set language '"true"'`.
fn parse_setting_value(raw: &str) -> serde_json::Value {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(value) => value,
        Err(_) => serde_json::Value::String(raw.to_string()),
    }
}

pub fn handle_config(action: &ConfigAction, cwd: &Path) -> Result<()> {
    let roots = coco_agent_host::paths::settings_roots_for_cwd(cwd);
    let settings = coco_config::settings::load_settings_for_roots(&roots, None)?;
    let json = serde_json::to_value(&settings.merged)?;

    match action {
        ConfigAction::List => {
            let pretty = serde_json::to_string_pretty(&json)?;
            println!("{pretty}");
        }
        ConfigAction::Get { key } => {
            if let Some(value) = json.get(key) {
                let pretty = serde_json::to_string_pretty(value)?;
                println!("{key} = {pretty}");
            } else {
                println!("Key '{key}' not found in configuration.");
                println!("Available keys:");
                if let Some(obj) = json.as_object() {
                    for k in obj.keys() {
                        println!("  {k}");
                    }
                }
            }
        }
        ConfigAction::Set { key, value } => {
            let written = global_config::write_user_setting(key, parse_setting_value(value))?;
            println!("Set '{key}' in {}", written.display());
            println!("Takes effect in the next session.");
        }
        ConfigAction::Reset => {
            let user_path = global_config::user_settings_path();
            if user_path.exists() {
                std::fs::remove_file(&user_path)?;
                println!("Configuration reset to defaults.");
            } else {
                println!("No user configuration file to reset.");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "config.test.rs"]
mod tests;
