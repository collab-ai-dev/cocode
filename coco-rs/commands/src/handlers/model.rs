//! `/model` — open the provider-grouped model picker (no args) or
//! validate + persist a model id (with args).
//!
//! Without args, opens the `ModelPicker` overlay (with a role pill so any of
//! the nine [`coco_types::ModelRole`] slots can be edited). With args,
//! validates and persists the given model id against the builtin
//! [`coco_config::builtin_models_partial`] registry.

use async_trait::async_trait;

use crate::CommandHandler;
use crate::CommandResult;
use crate::DialogSpec;

pub struct ModelHandler;

#[async_trait]
impl CommandHandler for ModelHandler {
    /// No args → open the picker overlay. Args → resolve `args` against
    /// the builtin registry, persist to
    /// `~/.coco/settings.json::models.main`, and report inline.
    async fn execute_command(&self, args: &str) -> crate::Result<CommandResult> {
        let requested = args.trim();
        if requested.is_empty() {
            return Ok(CommandResult::OpenDialog(DialogSpec::ModelPicker));
        }
        Ok(CommandResult::Text(handle_with_args(requested)))
    }

    fn handler_name(&self) -> &str {
        "model"
    }
}

/// Resolve `requested` against the builtin registry, persist to
/// `models.main`, and return a user-facing summary line. Pure
/// (no overlay side effects) so the typed-arg path stays inline.
fn handle_with_args(requested: &str) -> String {
    match resolve_model(requested) {
        Some(resolved) => {
            let selection = format!("{}/{}", resolved.provider, resolved.model_id);
            match coco_config::global_config::write_user_setting(
                "models.main",
                serde_json::Value::String(selection),
            ) {
                Ok(path) => format!(
                    "Set Main → {}/{} (persisted to {})\n  {}",
                    resolved.provider,
                    resolved.model_id,
                    path.display(),
                    resolved.summary
                ),
                Err(err) => format!(
                    "Set Main → {}/{} (failed to persist: {err})",
                    resolved.provider, resolved.model_id
                ),
            }
        }
        None => {
            let mut out = format!("Unknown model: {requested}\n\n");
            out.push_str("Use /model with no arguments to open the picker, or pick one of:\n");
            for entry in builtin_summary() {
                out.push_str(&format!(
                    "  {:<24}  {}\n",
                    format!("{}/{}", entry.provider, entry.model_id),
                    entry.summary
                ));
            }
            out
        }
    }
}

struct ResolvedBuiltin {
    provider: &'static str,
    model_id: String,
    summary: String,
}

/// Resolve `input` against the builtin registry. Matches in this
/// order: alias (sonnet/opus/haiku/gpt5/gemini-pro/deepseek-pro), exact
/// model_id, case-insensitive model_id, prefix.
fn resolve_model(input: &str) -> Option<ResolvedBuiltin> {
    let lower = input.to_ascii_lowercase();
    let alias_match: Option<&str> = match lower.as_str() {
        "sonnet" => Some("claude-sonnet-4-6"),
        "opus" => Some("claude-opus-4-7"),
        "haiku" => Some("claude-haiku-4-5"),
        "gpt5" | "gpt-5" => Some("gpt-5-4"),
        "gemini-pro" => Some("gemini-3.1-pro-preview"),
        "deepseek-pro" => Some("deepseek-v4-pro"),
        _ => None,
    };
    let target_id: Option<String> = alias_match.map(str::to_string).or_else(|| {
        let registry = coco_config::builtin_models_partial();
        if registry.contains_key(&lower) {
            Some(lower.clone())
        } else {
            registry
                .keys()
                .find(|k| k.eq_ignore_ascii_case(&lower) || k.starts_with(&lower))
                .cloned()
        }
    });
    let id = target_id?;
    let (provider, provider_display) = infer_provider(&id);
    let registry = coco_config::builtin_models_partial();
    let entry = registry.get(&id)?;
    let display_name = entry.display_name.clone().unwrap_or_else(|| id.clone());
    let ctx = entry
        .context_window
        .map(|t| format_context(t.get() as i64))
        .unwrap_or_else(|| "?".to_string());
    let thinking = if entry.supported_thinking_levels.is_some() {
        " · thinking"
    } else {
        ""
    };
    Some(ResolvedBuiltin {
        provider,
        model_id: id,
        summary: format!("{provider_display} · {display_name} · {ctx}{thinking}"),
    })
}

fn builtin_summary() -> Vec<ResolvedBuiltin> {
    let registry = coco_config::builtin_models_partial();
    let mut entries: Vec<ResolvedBuiltin> = registry
        .iter()
        .map(|(id, partial)| {
            let (provider, provider_display) = infer_provider(id);
            let display_name = partial.display_name.clone().unwrap_or_else(|| id.clone());
            let ctx = partial
                .context_window
                .map(|t| format_context(t.get() as i64))
                .unwrap_or_else(|| "?".to_string());
            let thinking = if partial.supported_thinking_levels.is_some() {
                " · thinking"
            } else {
                ""
            };
            ResolvedBuiltin {
                provider,
                model_id: id.clone(),
                summary: format!("{provider_display} · {display_name} · {ctx}{thinking}"),
            }
        })
        .collect();
    entries.sort_by(|a, b| {
        a.provider
            .cmp(b.provider)
            .then_with(|| a.model_id.cmp(&b.model_id))
    });
    entries
}

/// Mirror of `coco_tui::update::show::infer_provider`. Both must stay
/// in sync if a new provider lands.
fn infer_provider(model_id: &str) -> (&'static str, &'static str) {
    if model_id.starts_with("claude-") {
        ("anthropic", "Anthropic")
    } else if model_id.starts_with("gpt-") || model_id.starts_with('o') {
        ("openai", "OpenAI")
    } else if model_id.starts_with("gemini-") {
        ("google", "Google")
    } else if model_id.starts_with("deepseek-") {
        ("deepseek", "DeepSeek")
    } else {
        ("other", "Other")
    }
}

fn format_context(tokens: i64) -> String {
    if tokens >= 1_000_000 {
        let m = tokens as f64 / 1_000_000.0;
        if (m - m.round()).abs() < 0.05 {
            format!("{}M", m.round() as i64)
        } else {
            format!("{m:.1}M")
        }
    } else if tokens >= 1_000 {
        format!("{}K", tokens / 1_000)
    } else {
        format!("{tokens}")
    }
}

#[cfg(test)]
#[path = "model.test.rs"]
mod tests;
