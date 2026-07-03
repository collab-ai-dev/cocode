//! `/voice-config` — inspect and edit voice-input (speech-to-text) settings.
//!
//! Subsumes the former `/voice-lang` (now `/voice-config lang`). No-arg prints
//! a status summary; sub-commands persist one field each to user settings.json
//! and take effect on the next session (matching `write_user_setting`'s reload
//! semantics). `download` reports weight status — the fetch itself happens in
//! the voice engine (checksum-verified auto-download on first local use). On/off
//! stays on `/voice`.

use std::path::PathBuf;
use std::pin::Pin;

use coco_config::VoiceBackend;
use coco_config::VoiceConfig;

/// A short, illustrative set of common language codes for hints. Not an
/// allowlist — any well-formed BCP-47 / ISO-639-1 code (and `auto`) is accepted.
const COMMON_CODES: &[&str] = &["auto", "en", "es", "zh", "ja", "fr", "de", "pt", "ru", "ko"];

pub fn handler(
    args: String,
) -> Pin<Box<dyn std::future::Future<Output = crate::Result<String>> + Send>> {
    Box::pin(async move { run(&args) })
}

fn run(args: &str) -> crate::Result<String> {
    let mut parts = args.split_whitespace();
    let Some(sub) = parts.next() else {
        return Ok(summary());
    };
    let rest: Vec<&str> = parts.collect();
    match sub.to_ascii_lowercase().as_str() {
        "lang" | "language" => set_language(&rest),
        "backend" => set_backend(&rest),
        "remote" => set_remote(&rest),
        "local" => set_local(&rest),
        "download" => download_hint(&rest),
        "show" | "status" => Ok(summary()),
        other => Ok(format!("Unknown subcommand: {other}.\n\n{}", usage())),
    }
}

fn usage() -> String {
    "Usage:\n  \
     /voice-config                       show current settings\n  \
     /voice-config lang <code|auto>      dictation language\n  \
     /voice-config backend <remote|local>\n  \
     /voice-config remote <provider> [model]\n  \
     /voice-config local model <name>    whisper model (e.g. base.en, small)\n  \
     /voice-config local url <url|none>  custom weights URL override\n  \
     /voice-config local base <url|none> weights mirror base URL\n  \
     /voice-config download [model]      show weight-download status"
        .to_string()
}

fn set_language(rest: &[&str]) -> crate::Result<String> {
    let Some(code) = rest.first() else {
        return Ok(format!(
            "Dictation language: {}. Set it with `/voice-config lang <code>` \
             — e.g. {}, or `auto` to detect.",
            view().language,
            COMMON_CODES.join(", ")
        ));
    };
    let normalized = code.to_ascii_lowercase();
    if !is_valid_language(&normalized) {
        return Ok(format!(
            "Unsupported language code: {code}. Keeping {}. \
             Use an ISO-639-1 code (e.g. en, es, zh) or `auto`.",
            view().language
        ));
    }
    persist(
        "voice.language",
        serde_json::Value::String(normalized.clone()),
        || {
            if normalized == "auto" {
                "Dictation language set to auto (detect from speech).".to_string()
            } else {
                format!("Dictation language set to {normalized}.")
            }
        },
    )
}

fn set_backend(rest: &[&str]) -> crate::Result<String> {
    let Some(token) = rest.first() else {
        return Ok("Usage: /voice-config backend <remote|local>.".to_string());
    };
    let Some(backend) = VoiceBackend::from_token(token) else {
        return Ok(format!(
            "Unknown backend: {token}. Use `remote` (cloud OpenAI-wire) or \
             `local` (on-device Whisper)."
        ));
    };
    persist(
        "voice.backend",
        serde_json::Value::String(backend.as_str().to_string()),
        || match backend {
            VoiceBackend::Remote => {
                "Voice backend set to remote (cloud transcription).".to_string()
            }
            VoiceBackend::Local => concat!(
                "Voice backend set to local (on-device Whisper). Requires a ",
                "build with the `local-voice` feature; ensure the model is ",
                "downloaded with /voice-config download."
            )
            .to_string(),
        },
    )
}

fn set_remote(rest: &[&str]) -> crate::Result<String> {
    let Some(provider) = rest.first() else {
        let v = view();
        return Ok(format!(
            "Remote transcription: provider `{}`, model `{}`. Set with \
             `/voice-config remote <provider> [model]` — provider must be a \
             configured OpenAI-wire provider (e.g. openai, groq).",
            v.remote.provider, v.remote.model
        ));
    };
    if let Err(err) = coco_config::global_config::write_user_setting(
        "voice.remote.provider",
        serde_json::Value::String((*provider).to_string()),
    ) {
        return Ok(persist_err("voice.remote.provider", &err));
    }
    let mut msg = format!("Remote provider set to {provider}.");
    if let Some(model) = rest.get(1) {
        match coco_config::global_config::write_user_setting(
            "voice.remote.model",
            serde_json::Value::String((*model).to_string()),
        ) {
            Ok(_) => msg.push_str(&format!(" Model set to {model}.")),
            Err(err) => return Ok(persist_err("voice.remote.model", &err)),
        }
    }
    Ok(msg)
}

fn set_local(rest: &[&str]) -> crate::Result<String> {
    let Some(field) = rest.first() else {
        return Ok("Usage: /voice-config local <model|url|base> <value>. \
             e.g. `local model small`, `local url <weights-url>`."
            .to_string());
    };
    let value = rest.get(1).copied();
    match field.to_ascii_lowercase().as_str() {
        "model" => {
            let Some(name) = value else {
                return Ok(
                    "Usage: /voice-config local model <name> (e.g. base.en, small).".to_string(),
                );
            };
            persist(
                "voice.local.whisper.model",
                serde_json::Value::String(name.to_string()),
                || format!("Whisper model set to {name}."),
            )
        }
        "url" => set_optional_url("voice.local.whisper.model_url", "custom weights URL", value),
        "base" => set_optional_url(
            "voice.local.whisper.download_base",
            "weights mirror base",
            value,
        ),
        other => Ok(format!(
            "Unknown local field: {other}. Use `model`, `url`, or `base`."
        )),
    }
}

/// Persist an optional URL-ish field. `none`/`default`/`clear` (or a missing
/// value) resets it to the built-in default by writing JSON `null`.
fn set_optional_url(key: &str, label: &str, value: Option<&str>) -> crate::Result<String> {
    match value {
        None | Some("none") | Some("default") | Some("clear") => {
            persist(key, serde_json::Value::Null, || {
                format!("Reset {label} to the built-in default.")
            })
        }
        Some(url) => persist(key, serde_json::Value::String(url.to_string()), || {
            format!("Set {label} to {url}.")
        }),
    }
}

/// Report weight-download status. The download itself happens in the voice
/// engine (which owns the runtime + progress stream) — a known model is fetched
/// and checksum-verified automatically on first use. This command reports where
/// the weights live and whether they are already present.
fn download_hint(rest: &[&str]) -> crate::Result<String> {
    let v = view();
    let whisper = &v.local.whisper;
    let model = match rest.first() {
        Some(m) => *m,
        None => whisper.model.as_str(),
    };
    // Resolve the path for the *reported* model (the arg, if given), not the
    // configured one, so status and name agree.
    let path = whisper_model_path(model, whisper.cache_dir.as_ref());
    if path.exists() {
        return Ok(format!(
            "Whisper model `{model}` is already present at {}.",
            path.display()
        ));
    }
    Ok(format!(
        "Whisper model `{model}` is not downloaded yet. It downloads (and \
         checksum-verifies) automatically to {} on first use: switch to the \
         local backend with `/voice-config backend local`, then press F3 and \
         speak — progress shows in the input hint.",
        path.display()
    ))
}

/// Build the no-arg status summary from persisted settings + defaults.
fn summary() -> String {
    let v = view();
    let whisper = &v.local.whisper;
    let path = whisper_model_path(&whisper.model, whisper.cache_dir.as_ref());
    let weights = if path.exists() {
        format!("downloaded ({})", path.display())
    } else {
        "not downloaded (auto-downloads on first local use)".to_string()
    };
    format!(
        "Voice input settings (changes apply on the next session):\n  \
         backend    {backend}   (/voice-config backend <remote|local>)\n  \
         language   {language}   (/voice-config lang <code|auto>)\n  \
         remote     {rprovider} · {rmodel}   (/voice-config remote <provider> [model])\n  \
         local      whisper · {wmodel}   (/voice-config local model <name>)\n  \
         weights    {weights}\n\
         Toggle voice on/off with /voice.",
        backend = v.backend.as_str(),
        language = v.language,
        rprovider = v.remote.provider,
        rmodel = v.remote.model,
        wmodel = whisper.model,
    )
}

/// Effective voice config: persisted user settings applied over defaults.
/// (Env overrides and project layers are not reflected — this reads the file
/// the sub-commands write, so it round-trips what the user sees here.)
fn view() -> VoiceConfig {
    let defaults = VoiceConfig::default();
    let path = coco_config::global_config::user_settings_path();
    let Some(settings) = std::fs::read_to_string(path)
        .ok()
        .and_then(|c| coco_config::parse_settings(&c).ok())
    else {
        return defaults;
    };
    let v = settings.voice;
    VoiceConfig {
        backend: v.backend.unwrap_or(defaults.backend),
        language: v.language.unwrap_or(defaults.language),
        transcript_mode: v.transcript_mode.unwrap_or(defaults.transcript_mode),
        remote: v.remote.unwrap_or(defaults.remote),
        local: v.local.unwrap_or(defaults.local),
    }
}

/// `<cache_dir>/ggml-<model>.bin`, defaulting the cache dir to
/// `<config_home>/models/whisper/`. Mirrors `coco_voice::resolve_model_path`
/// (the voice crate is not a dependency here).
fn whisper_model_path(model: &str, cache_dir: Option<&PathBuf>) -> PathBuf {
    let dir = cache_dir.cloned().unwrap_or_else(|| {
        coco_config::global_config::config_home()
            .join("models")
            .join("whisper")
    });
    dir.join(format!("ggml-{model}.bin"))
}

/// Persist `key = value`; on success run `ok_msg`, else a uniform error hint.
fn persist(
    key: &str,
    value: serde_json::Value,
    ok_msg: impl FnOnce() -> String,
) -> crate::Result<String> {
    match coco_config::global_config::write_user_setting(key, value) {
        Ok(_) => Ok(ok_msg()),
        Err(err) => Ok(persist_err(key, &err)),
    }
}

fn persist_err(key: &str, err: &impl std::fmt::Display) -> String {
    format!(
        "Failed to persist {key}: {err}. You can set it manually as `{key}` \
         in settings.json."
    )
}

/// `auto`, or a 2-letter language optionally with region/script subtags
/// (`en`, `zh`, `pt-br`, `zh-hant`). Deliberately permissive.
fn is_valid_language(code: &str) -> bool {
    if code == "auto" {
        return true;
    }
    let mut segs = code.split('-');
    let Some(primary) = segs.next() else {
        return false;
    };
    if primary.len() != 2 || !primary.chars().all(|c| c.is_ascii_lowercase()) {
        return false;
    }
    segs.all(|sub| (2..=4).contains(&sub.len()) && sub.chars().all(|c| c.is_ascii_alphanumeric()))
}

#[cfg(test)]
#[path = "voice_config.test.rs"]
mod tests;
