//! Deterministic, warn-only quality audit for compact summaries.
//!
//! Detects the "literal summarizer" failure class observed with
//! non-Claude models: the trailing summarization request echoed into the
//! summary (directive tags or verbatim prompt lines) and template
//! placeholders left unfilled. Detection never alters control flow —
//! `call_with_ptl_retry` logs one warn per anomaly so per-provider echo
//! rates are visible in telemetry before any corrective machinery
//! (retry, scrub-by-fingerprint) is justified.

use crate::prompt::COMPACT_DIRECTIVE_CLOSE;
use crate::prompt::COMPACT_DIRECTIVE_OPEN;
use crate::prompt::DIRECTIVE_BODY_MARKERS;

/// Minimum trimmed-line length (bytes) for a request line to act as an
/// echo fingerprint. Section headers ("6. All user messages:")
/// legitimately reappear in summaries and stay under this floor; the
/// long instruction clauses never legitimately reappear.
const ECHO_FINGERPRINT_MIN_LINE_BYTES: usize = 48;

/// Verbatim fingerprint-line hits at which the summary counts as a
/// prompt echo.
const ECHO_MIN_MATCHED_LINES: usize = 3;

/// Matched-byte volume backstop for requests with few long lines
/// (e.g. echo dominated by custom instructions).
const ECHO_MIN_MATCHED_BYTES: usize = 600;

/// Placeholder-only lines at which sections count as unfilled.
const PLACEHOLDER_MIN_LINES: usize = 2;

/// One detected summary-quality anomaly. Telemetry only — the summary
/// is still accepted (a degraded summary beats a failed compaction when
/// the context is nearly full).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactSummaryAnomaly {
    /// Output contains the directive sentinel tag or the NO_TOOLS
    /// "CRITICAL:" line — the instruction was echoed recognizably.
    DirectiveEcho,
    /// Long request lines reproduced verbatim — catches tag-stripped
    /// echo and custom-instruction echo the sentinel check misses.
    PromptEcho,
    /// Lines that are solely bracketed template placeholders
    /// ("[Task 1]") — sections were left unfilled.
    PlaceholderSection,
}

impl CompactSummaryAnomaly {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DirectiveEcho => "directive_echo",
            Self::PromptEcho => "prompt_echo",
            Self::PlaceholderSection => "placeholder_section",
        }
    }
}

/// Audit a raw summarizer response against the request that produced it.
/// Pure string analysis; returns at most one instance of each variant.
pub fn detect_compact_summary_anomalies(
    summary: &str,
    summary_request: &str,
) -> Vec<CompactSummaryAnomaly> {
    let mut anomalies = Vec::new();

    // Either sentinel alone counts: a tail-only echo (FINAL REMINDER +
    // close tag) carries no open tag, and vice versa for truncation.
    if summary.contains(COMPACT_DIRECTIVE_OPEN)
        || summary.contains(COMPACT_DIRECTIVE_CLOSE)
        || DIRECTIVE_BODY_MARKERS.iter().any(|m| summary.contains(m))
    {
        anomalies.push(CompactSummaryAnomaly::DirectiveEcho);
    }

    let fingerprints: std::collections::HashSet<&str> = summary_request
        .lines()
        .map(str::trim)
        .filter(|line| line.len() >= ECHO_FINGERPRINT_MIN_LINE_BYTES)
        .collect();
    let mut matched_lines = 0;
    let mut matched_bytes = 0;
    for line in summary.lines() {
        let trimmed = line.trim();
        if fingerprints.contains(trimmed) {
            matched_lines += 1;
            matched_bytes += trimmed.len();
        }
    }
    if matched_lines >= ECHO_MIN_MATCHED_LINES || matched_bytes >= ECHO_MIN_MATCHED_BYTES {
        anomalies.push(CompactSummaryAnomaly::PromptEcho);
    }

    let placeholder_lines = summary.lines().filter(|l| is_placeholder_line(l)).count();
    if placeholder_lines >= PLACEHOLDER_MIN_LINES {
        anomalies.push(CompactSummaryAnomaly::PlaceholderSection);
    }

    anomalies
}

/// A line that is solely a bracketed template placeholder, optionally
/// bulleted: "- [Task 1]", "[Precise description of current work]",
/// "[...]". Markdown checkboxes ("- [ ] item", "- [x] item"), numeric
/// citations ("[1]") and markdown links are not placeholders.
fn is_placeholder_line(line: &str) -> bool {
    let trimmed = line.trim();
    let body = trimmed.strip_prefix("- ").unwrap_or(trimmed).trim();
    let Some(inner) = body
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    else {
        return false;
    };
    let inner = inner.trim();
    if inner == "..." {
        return true;
    }
    // Markdown checkbox ("[ ]" / "[x]") — not a placeholder.
    if inner.is_empty() || inner.eq_ignore_ascii_case("x") {
        return false;
    }
    // Numeric citation ("[1]") — not a placeholder.
    if inner.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    inner.chars().any(char::is_alphabetic)
}

#[cfg(test)]
#[path = "summary_guard.test.rs"]
mod tests;
