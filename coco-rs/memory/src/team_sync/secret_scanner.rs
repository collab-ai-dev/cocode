//! Client-side secret scanner for team memory.
//!
//! Wraps [`coco_secret_redact::scan_secrets`] to project the broader
//! redaction taxonomy onto the team-memory-specific
//! [`SkippedSecretFile`] payload. Catches the high-confidence
//! prefix-based credentials (Anthropic / OpenAI / GitHub / Slack /
//! AWS / generic Bearer / key-assignment).
//!
//! Uses gitleaks rule IDs; coco-rs maps the underlying
//! `coco_secret_redact::SecretMatch.rule_id` strings (`"anthropic"`,
//! `"github-token"`, etc.) to canonical IDs so the cross-language
//! analytics keys match.

use super::types::SkippedSecretFile;

/// Map a `coco_secret_redact` rule id to the gitleaks rule id.
/// IDs are kebab-case and reused across the analytics surface;
/// keeping the mapping centralised here means the broader
/// `coco_secret_redact` redaction tooling can grow new patterns
/// without churn here.
fn map_rule_id(internal: &str) -> &'static str {
    match internal {
        "anthropic" => "anthropic-api-key",
        "openai" => "openai-api-key",
        "github-token" => "github-pat",
        "slack-token" => "slack-app-token",
        "aws-access-key" => "aws-access-token",
        "bearer-token" => "bearer-token",
        // Stable fallback — analytics will see the raw ID; not ideal
        // but won't drop the detection.
        _ => "secret",
    }
}

fn label_from_rule(rule_id: &str) -> String {
    rule_id
        .split('-')
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Scan `content` for credentials. Returns the first matching
/// `SkippedSecretFile` (short-circuits on first detection since one
/// secret is enough to skip the file).
///
/// `path` is the relative path under the team memory dir, recorded
/// on the `SkippedSecretFile` so the caller can report which file was
/// rejected.
pub fn scan_for_secrets(path: &str, content: &str) -> Option<SkippedSecretFile> {
    let matches = coco_secret_redact::scan_secrets(content);
    let first = matches.first()?;
    let rule_id = map_rule_id(first.rule_id);
    Some(SkippedSecretFile {
        path: path.to_string(),
        rule_id: rule_id.to_string(),
        label: label_from_rule(rule_id),
    })
}

#[cfg(test)]
#[path = "secret_scanner.test.rs"]
mod tests;
