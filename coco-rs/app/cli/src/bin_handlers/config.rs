//! `coco config <action>` — get/set/list/reset for user settings.

use anyhow::Result;
use coco_cli::ConfigAction;
use coco_config::global_config;

pub fn handle_config(action: &ConfigAction) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let settings = coco_config::settings::load_settings(&cwd, None)?;
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
            let user_path = global_config::user_settings_path();
            println!("Would set '{key}' = '{value}' in {}", user_path.display());
            println!(
                "Settings file: {}",
                if user_path.exists() {
                    "exists"
                } else {
                    "will be created"
                }
            );
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
