use std::collections::BTreeSet;

use crate::schema::ModelFamily;
use crate::schema::OpenRouterModel;

pub(crate) fn aliases_for_openrouter_model(model: &OpenRouterModel) -> Vec<String> {
    let mut aliases = BTreeSet::new();
    let is_free_variant = model.id.ends_with(":free");
    if !is_free_variant {
        if let Some(slug) = &model.canonical_slug {
            aliases.insert(slug.clone());
        }
        if let Some(hf) = &model.hugging_face_id {
            aliases.insert(hf.clone());
        }
    }
    if let Some(provider) = provider_from_id(&model.id) {
        aliases.insert(strip_provider(&model.id).to_string());
        if !is_free_variant && let Some(slug) = &model.canonical_slug {
            aliases.insert(strip_provider(slug).to_string());
        }
        if provider == "anthropic" {
            aliases.extend(anthropic_aliases(&model.id));
            if !is_free_variant && let Some(slug) = &model.canonical_slug {
                aliases.extend(anthropic_aliases(slug));
            }
        }
    }
    aliases.remove(&model.id);
    aliases.into_iter().collect()
}

fn anthropic_aliases(id: &str) -> Vec<String> {
    let providerless = strip_provider(id);
    let normalized = normalize_id(providerless);
    let Some((version, family, modifier)) = anthropic_version_family(&normalized) else {
        return Vec::new();
    };
    let suffix = modifier.map_or_else(String::new, |m| format!("-{m}"));
    vec![
        format!("claude-{family}-{version}{suffix}"),
        format!("claude-{version}-{family}{suffix}"),
        format!("{family}-{version}{suffix}"),
    ]
}

fn anthropic_version_family(normalized: &str) -> Option<(String, String, Option<String>)> {
    let tokens: Vec<&str> = normalized.split('-').collect();
    let family = ["opus", "sonnet", "haiku"]
        .into_iter()
        .find(|candidate| tokens.contains(candidate))?;
    let idx = tokens.iter().position(|token| *token == family)?;
    let version = if idx >= 2
        && tokens.get(idx - 2) == Some(&"4")
        && tokens
            .get(idx - 1)
            .is_some_and(|part| part.chars().all(|c| c.is_ascii_digit()))
    {
        Some((format!("4-{}", tokens[idx - 1]), idx + 1))
    } else if tokens.get(idx + 1) == Some(&"4")
        && tokens
            .get(idx + 2)
            .is_some_and(|part| part.chars().all(|c| c.is_ascii_digit()))
    {
        Some((format!("4-{}", tokens[idx + 2]), idx + 3))
    } else {
        None
    }?;
    let modifier = tokens
        .get(version.1)
        .filter(|token| **token == "fast")
        .map(|token| (*token).to_string());
    Some((version.0, family.to_string(), modifier))
}

pub(crate) fn lookup_key_tiers(provider: Option<&str>, model_id: &str) -> Vec<BTreeSet<String>> {
    let raw = model_id.trim();
    if raw.is_empty() {
        return Vec::new();
    }

    let normalized = normalize_id(raw);
    let providerless = strip_provider(&normalized);
    let stripped_date = strip_trailing_date(providerless);

    let mut exact = BTreeSet::new();
    exact.insert(normalized.clone());
    if let Some(provider) = provider {
        let provider = normalize_id(provider);
        exact.insert(format!("{provider}/{providerless}"));
    }

    let mut providerless_tier = BTreeSet::new();
    providerless_tier.insert(providerless.to_string());

    let mut stripped = BTreeSet::new();
    stripped.insert(stripped_date.clone());
    if let Some(provider) = provider {
        let provider = normalize_id(provider);
        stripped.insert(format!("{provider}/{stripped_date}"));
    }

    vec![exact, providerless_tier, stripped]
}

pub(crate) fn normalize_id(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut prev_dash = false;
    for ch in input.trim().chars() {
        let mapped = match ch {
            '_' | '.' | ' ' => '-',
            c => c.to_ascii_lowercase(),
        };
        if mapped == '-' {
            if !prev_dash {
                out.push(mapped);
            }
            prev_dash = true;
        } else {
            out.push(mapped);
            prev_dash = false;
        }
    }
    out.trim_matches('-').to_string()
}

pub(crate) fn strip_provider(input: &str) -> &str {
    input.rsplit_once('/').map_or(input, |(_, model)| model)
}

pub(crate) fn provider_from_id(id: &str) -> Option<String> {
    id.split_once('/').map(|(provider, _)| {
        provider
            .trim_start_matches('~')
            .to_ascii_lowercase()
            .replace('_', "-")
    })
}

fn strip_trailing_date(input: &str) -> String {
    let trimmed = input;
    if trimmed.len() >= 9 {
        let suffix = &trimmed[trimmed.len() - 9..];
        if suffix.starts_with('-') && suffix[1..].chars().all(|c| c.is_ascii_digit()) {
            return trimmed[..trimmed.len() - 9].to_string();
        }
    }
    if trimmed.len() >= 11 {
        let suffix = &trimmed[trimmed.len() - 11..];
        let bytes = suffix.as_bytes();
        if bytes[0] == b'-'
            && bytes[5] == b'-'
            && bytes[8] == b'-'
            && suffix
                .chars()
                .enumerate()
                .all(|(idx, ch)| matches!(idx, 0 | 5 | 8) || ch.is_ascii_digit())
        {
            return trimmed[..trimmed.len() - 11].to_string();
        }
    }
    trimmed.to_string()
}

pub(crate) fn family_for_model(
    provider: Option<&str>,
    id: &str,
    canonical_slug: Option<&str>,
) -> ModelFamily {
    let haystack = canonical_slug.map_or_else(|| normalize_id(id), normalize_id);
    match provider {
        Some("anthropic") => ModelFamily::Claude,
        Some("openai") => ModelFamily::Gpt,
        Some("google") => ModelFamily::Gemini,
        Some("deepseek") => ModelFamily::DeepSeek,
        _ if haystack.contains("claude") => ModelFamily::Claude,
        _ if haystack.contains("gpt") || haystack.contains("o3") || haystack.contains("o4") => {
            ModelFamily::Gpt
        }
        _ if haystack.contains("gemini") || haystack.contains("gemma") => ModelFamily::Gemini,
        _ if haystack.contains("deepseek") => ModelFamily::DeepSeek,
        _ => ModelFamily::Other,
    }
}
