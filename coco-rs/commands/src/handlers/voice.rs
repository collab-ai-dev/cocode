//! `/voice` — enable/disable voice input (speech-to-text dictation).
//!
//! Persists `features.voice` to user settings.json (`Feature::Voice` is the
//! single on/off truth — there is no `voice.enabled` field). In the TUI the
//! command is additionally intercepted live by `tui_runner` so the keybinding
//! gate + footer update within the session; this registry handler is the
//! headless/SDK path and the persistence layer.

use std::pin::Pin;

pub fn handler(
    args: String,
) -> Pin<Box<dyn std::future::Future<Output = crate::Result<String>> + Send>> {
    Box::pin(async move { run(&args) })
}

fn run(args: &str) -> crate::Result<String> {
    let target = match args.trim().to_ascii_lowercase().as_str() {
        "" | "toggle" => !current_enabled(),
        "on" | "enable" => true,
        "off" | "disable" => false,
        other => {
            return Ok(format!(
                "Unknown argument: {other}. Usage: /voice [on|off|toggle]."
            ));
        }
    };

    match coco_config::global_config::write_user_setting(
        "features.voice",
        serde_json::Value::Bool(target),
    ) {
        Ok(_) if target => Ok(concat!(
            "Voice input enabled. Press F3 in the terminal UI to record ",
            "(rebindable via /keybindings). Configure it with /voice-config."
        )
        .to_string()),
        Ok(_) => Ok("Voice input disabled.".to_string()),
        Err(err) => Ok(format!(
            "Failed to persist voice setting: {err}. \
             You can set it manually as `features.voice` in settings.json."
        )),
    }
}

/// Best-effort read of the persisted `features.voice` flag (defaults to off).
/// The live TUI toggle uses authoritative runtime state instead — this only
/// backs the headless `toggle` branch.
fn current_enabled() -> bool {
    let path = coco_config::global_config::user_settings_path();
    let Ok(contents) = std::fs::read_to_string(path) else {
        return false;
    };
    coco_config::parse_settings(&contents)
        .ok()
        .and_then(|s| s.features.get("voice").copied())
        .unwrap_or(false)
}

#[cfg(test)]
#[path = "voice.test.rs"]
mod tests;
