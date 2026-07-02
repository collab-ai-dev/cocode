//! `/voice-lang` — choose the dictation language.
//!
//! No-arg lists the current language + a hint (promotable to a modal picker in
//! a later phase without changing this command's name or the with-arg path).
//! With-arg loosely validates a BCP-47 / ISO-639-1 code (or `auto`) and
//! persists `voice.language` to user settings.json.

use std::pin::Pin;

/// A short, illustrative set of common codes for the no-arg hint. Not an
/// allowlist — any well-formed code (and `auto`) is accepted.
const COMMON_CODES: &[&str] = &["auto", "en", "es", "zh", "ja", "fr", "de", "pt", "ru", "ko"];

pub fn handler(
    args: String,
) -> Pin<Box<dyn std::future::Future<Output = crate::Result<String>> + Send>> {
    Box::pin(async move { run(&args) })
}

fn run(args: &str) -> crate::Result<String> {
    let arg = args.trim();
    if arg.is_empty() {
        let current = current_language();
        return Ok(format!(
            "Dictation language: {current}. \
             Set it with `/voice-lang <code>` — e.g. {}, or `auto` to detect.",
            COMMON_CODES.join(", ")
        ));
    }

    let normalized = arg.to_ascii_lowercase();
    if !is_valid_language(&normalized) {
        return Ok(format!(
            "Unsupported language code: {arg}. Keeping {}. \
             Use an ISO-639-1 code (e.g. en, es, zh) or `auto`.",
            current_language()
        ));
    }

    match coco_config::global_config::write_user_setting(
        "voice.language",
        serde_json::Value::String(normalized.clone()),
    ) {
        Ok(_) if normalized == "auto" => {
            Ok("Dictation language set to auto (detect from speech).".to_string())
        }
        Ok(_) => Ok(format!("Dictation language set to {normalized}.")),
        Err(err) => Ok(format!(
            "Failed to persist language: {err}. \
             You can set it manually as `voice.language` in settings.json."
        )),
    }
}

/// `auto`, or a 2-letter language optionally with a region/script subtag
/// (`en`, `zh`, `pt-br`, `zh-hant`). Deliberately permissive.
fn is_valid_language(code: &str) -> bool {
    if code == "auto" {
        return true;
    }
    let mut parts = code.split('-');
    let Some(primary) = parts.next() else {
        return false;
    };
    if primary.len() != 2 || !primary.chars().all(|c| c.is_ascii_lowercase()) {
        return false;
    }
    parts.all(|sub| (2..=4).contains(&sub.len()) && sub.chars().all(|c| c.is_ascii_alphanumeric()))
}

fn current_language() -> String {
    let path = coco_config::global_config::user_settings_path();
    std::fs::read_to_string(path)
        .ok()
        .and_then(|contents| coco_config::parse_settings(&contents).ok())
        .and_then(|s| s.voice.language)
        .unwrap_or_else(|| "auto".to_string())
}

#[cfg(test)]
#[path = "voice_lang.test.rs"]
mod tests;
