//! Best-effort secret redaction for tool output.
//!
//! Applies well-known regex patterns to strip API keys, bearer tokens,
//! AWS access keys, and generic secret assignments from strings before
//! they enter model context or logs.

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::Regex;

// Provider-specific key patterns.
static OPENAI_KEY_REGEX: LazyLock<Regex> = LazyLock::new(|| compile_regex(r"sk-[A-Za-z0-9]{20,}"));

// Must be applied BEFORE OPENAI_KEY_REGEX because `sk-ant-...` would
// otherwise be partially matched by the `sk-...` pattern.
static ANTHROPIC_KEY_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"sk-ant-[A-Za-z0-9\-]{20,}"));

static GITHUB_TOKEN_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"\b(ghp|ghs|gho|ghu|ghr|github_pat)_[A-Za-z0-9]{20,}\b"));

static SLACK_TOKEN_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"\bxox[bpras]-[A-Za-z0-9\-]{10,}\b"));

static AWS_ACCESS_KEY_ID_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"\bAKIA[0-9A-Z]{16}\b"));

// Generic patterns.
static BEARER_TOKEN_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"(?i)\bBearer\s+[A-Za-z0-9._\-]{16,}\b"));

static SECRET_ASSIGNMENT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    compile_regex(r#"(?i)\b(api[_-]?key|token|secret|password)\b(\s*[:=]\s*)(["']?)[^\s"']{8,}"#)
});

/// Placeholder inserted in place of detected secrets.
const REDACTED: &str = "[REDACTED_SECRET]";
const BEARER_REPLACEMENT: &str = "Bearer [REDACTED_SECRET]";
const ASSIGNMENT_REPLACEMENT: &str = "$1$2$3[REDACTED_SECRET]";

/// Apply a regex replacement, only allocating when there is a match.
fn apply_regex<'a>(input: Cow<'a, str>, regex: &Regex, replacement: &str) -> Cow<'a, str> {
    match regex.replace_all(&input, replacement) {
        Cow::Borrowed(_) => input,
        Cow::Owned(new) => Cow::Owned(new),
    }
}

/// A single secret match identified by [`scan_secrets`]. Carries the
/// rule label (e.g. `"anthropic"`, `"github-pat"`) so callers can build
/// user-facing reject messages without exposing the matched bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretMatch {
    /// Stable rule identifier — kebab-case.
    pub rule_id: &'static str,
    /// Byte offset of the match in the original input.
    pub start: usize,
    /// End byte offset (exclusive).
    pub end: usize,
}

impl SecretMatch {
    /// Human-readable label derived from the rule ID. Used in TS
    /// `secretScanner.ts` for telemetry and reject-toast text.
    pub fn label(&self) -> String {
        self.rule_id
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
}

const RULES: &[(&str, &LazyLock<Regex>)] = &[
    ("anthropic", &ANTHROPIC_KEY_REGEX),
    ("openai", &OPENAI_KEY_REGEX),
    ("github-token", &GITHUB_TOKEN_REGEX),
    ("slack-token", &SLACK_TOKEN_REGEX),
    ("aws-access-key", &AWS_ACCESS_KEY_ID_REGEX),
    ("bearer-token", &BEARER_TOKEN_REGEX),
];

/// Detect (don't redact) secrets in `input`. Returns one
/// [`SecretMatch`] per detection so callers can BLOCK writes (rather
/// than silently redact) — used by the team-memory write guard.
///
/// Secrets must never leave the user's machine, so detected hits cause the
/// write to be rejected outright with a labeled reason.
///
/// Empty result is the no-op "safe to write" signal. For redaction
/// (when blocking is too aggressive), use [`redact_secrets`].
pub fn scan_secrets(input: &str) -> Vec<SecretMatch> {
    let mut matches = Vec::new();
    for (rule_id, regex) in RULES {
        for m in regex.find_iter(input) {
            matches.push(SecretMatch {
                rule_id,
                start: m.start(),
                end: m.end(),
            });
        }
    }
    matches
}

/// Remove secrets and keys from a string on a best-effort basis.
///
/// Returns the input unchanged (zero-copy) when no secrets are found.
///
/// Matches Anthropic keys (`sk-ant-...`), OpenAI keys (`sk-...`),
/// GitHub tokens (`ghp_...`, `github_pat_...`), Slack tokens (`xoxb-...`),
/// AWS access key IDs (`AKIA...`), Bearer tokens, and generic
/// `key=value` / `secret: value` assignments.
pub fn redact_secrets(input: &str) -> Cow<'_, str> {
    let s = Cow::Borrowed(input);
    let s = apply_regex(s, &ANTHROPIC_KEY_REGEX, REDACTED);
    let s = apply_regex(s, &OPENAI_KEY_REGEX, REDACTED);
    let s = apply_regex(s, &GITHUB_TOKEN_REGEX, REDACTED);
    let s = apply_regex(s, &SLACK_TOKEN_REGEX, REDACTED);
    let s = apply_regex(s, &AWS_ACCESS_KEY_ID_REGEX, REDACTED);
    let s = apply_regex(s, &BEARER_TOKEN_REGEX, BEARER_REPLACEMENT);
    apply_regex(s, &SECRET_ASSIGNMENT_REGEX, ASSIGNMENT_REPLACEMENT)
}

fn compile_regex(pattern: &str) -> Regex {
    match Regex::new(pattern) {
        Ok(regex) => regex,
        // Panic is acceptable: `load_regex` test catches this at build time.
        Err(err) => panic!("invalid regex pattern `{pattern}`: {err}"),
    }
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
