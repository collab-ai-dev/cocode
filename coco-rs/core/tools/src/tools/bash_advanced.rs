//! Advanced Bash tool features.
//!
//! Provides output processing (truncation with intelligent boundaries),
//! command classification (search/read/list/silent), progress tracking,
//! auto-backgrounding detection, CWD tracking, image output handling,
//! and command description extraction.

use std::collections::HashSet;
use std::sync::LazyLock;
use std::time::Instant;

/// Show progress spinner after this threshold.
const PROGRESS_THRESHOLD_MS: u64 = 2000;

/// Auto-background blocking commands after this budget in assistant mode.
pub(crate) const ASSISTANT_BLOCKING_BUDGET_MS: u64 = 15_000;

/// Commands that must NOT be auto-backgrounded on timeout — they belong in the
/// foreground unless the user explicitly backgrounds them.
const DISALLOWED_AUTO_BACKGROUND_COMMANDS: &[&str] = &["sleep"];

/// Whether `command` is eligible for automatic backgrounding on timeout.
/// Take the base command (first token) and allow everything except the
/// disallowed set.
pub(crate) fn is_autobackgrounding_allowed(command: &str) -> bool {
    match command.split_whitespace().next() {
        Some(base) => !DISALLOWED_AUTO_BACKGROUND_COMMANDS.contains(&base),
        None => true,
    }
}

/// Maximum image file size for base64 detection (20 MB).
const MAX_IMAGE_FILE_SIZE: usize = 20 * 1024 * 1024;

// ── Command classification sets ──

static SEMANTIC_NEUTRAL_COMMANDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    ["echo", "printf", "true", "false", ":"]
        .into_iter()
        .collect()
});

static SILENT_COMMANDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "mv", "cp", "rm", "mkdir", "rmdir", "chmod", "chown", "chgrp", "touch", "ln", "cd",
        "export", "unset", "wait",
    ]
    .into_iter()
    .collect()
});

/// Common long-running commands (for analytics/classification).
static COMMON_BACKGROUND_COMMANDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "npm",
        "yarn",
        "pnpm",
        "node",
        "python",
        "python3",
        "go",
        "cargo",
        "make",
        "docker",
        "terraform",
        "webpack",
        "vite",
        "jest",
        "pytest",
        "curl",
        "wget",
        "build",
        "test",
        "serve",
        "watch",
        "dev",
    ]
    .into_iter()
    .collect()
});

/// Check if a command is expected to produce no stdout on success.
pub fn is_silent_command(command: &str) -> bool {
    let parts = split_command_with_operators(command);
    if parts.is_empty() {
        return false;
    }

    let mut has_non_fallback = false;
    let mut last_operator: Option<&str> = None;
    let mut skip_next_as_redirect = false;

    for part in &parts {
        if skip_next_as_redirect {
            skip_next_as_redirect = false;
            continue;
        }

        match part.as_str() {
            ">" | ">>" | ">&" => {
                skip_next_as_redirect = true;
                continue;
            }
            op @ ("||" | "&&" | "|" | ";") => {
                last_operator = Some(op);
                continue;
            }
            _ => {}
        }

        let base_command = part.split_whitespace().next().unwrap_or("");
        if base_command.is_empty() {
            continue;
        }

        // Fallback commands after || (e.g., `rm file || echo "not found"`) are neutral
        if last_operator == Some("||") && SEMANTIC_NEUTRAL_COMMANDS.contains(base_command) {
            continue;
        }

        has_non_fallback = true;
        if !SILENT_COMMANDS.contains(base_command) {
            return false;
        }
    }

    has_non_fallback
}

// ── Auto-backgrounding ──

/// Detect standalone or leading `sleep N` patterns that should use Monitor.
///
/// Returns a description of the blocked pattern, or `None` if allowed.
pub fn detect_blocked_sleep_pattern(command: &str) -> Option<String> {
    let parts = split_simple(command);
    if parts.is_empty() {
        return None;
    }

    let first = parts[0].trim();
    // Match `sleep N` where N is an integer >= 2
    let rest_after_sleep = first.strip_prefix("sleep")?;
    let rest_after_sleep = rest_after_sleep.trim();
    let secs: u64 = rest_after_sleep.parse().ok()?;

    if secs < 2 {
        return None;
    }

    let rest: String = parts[1..].join(" ").trim().to_string();
    if rest.is_empty() {
        Some(format!("standalone sleep {secs}"))
    } else {
        Some(format!("sleep {secs} followed by: {rest}"))
    }
}

// Command-semantics interpretation lives in the single canonical
// `coco_shell::semantics::interpret_command_result`. A duplicate implementation
// previously lived here and was never wired — removed per the no-duplicate-helper rule.

// ── Image output detection ──

/// Check if content is a base64-encoded image data URL.
pub fn is_image_output(content: &str) -> bool {
    // Match `data:image/<subtype>;base64,`
    content.len() < MAX_IMAGE_FILE_SIZE
        && content.starts_with("data:image/")
        && content.contains(";base64,")
}

/// Parse a data-URI into media type and base64 payload.
pub fn parse_data_uri(s: &str) -> Option<(&str, &str)> {
    let s = s.trim();
    let rest = s.strip_prefix("data:")?;
    let semi_pos = rest.find(';')?;
    let media_type = &rest[..semi_pos];
    let after_semi = &rest[semi_pos + 1..];
    let payload = after_semi.strip_prefix("base64,")?;
    Some((media_type, payload))
}

// ── Progress tracking ──

/// Tracks progress for a running bash command.
#[derive(Debug)]
pub struct BashProgressTracker {
    start_time: Instant,
    last_output: String,
    total_lines: i64,
    total_bytes: i64,
    progress_emitted: bool,
}

impl Default for BashProgressTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl BashProgressTracker {
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
            last_output: String::new(),
            total_lines: 0,
            total_bytes: 0,
            progress_emitted: false,
        }
    }

    /// Update with new output data.
    pub fn update(&mut self, output: &str, total_bytes: i64) {
        self.last_output = output.to_string();
        self.total_lines = output.lines().count() as i64;
        self.total_bytes = total_bytes;
    }

    /// Whether we should emit a progress update (past the 2-second threshold).
    pub fn should_emit_progress(&self) -> bool {
        self.elapsed_ms() >= PROGRESS_THRESHOLD_MS
    }

    /// Whether we should auto-background (past the assistant blocking budget).
    pub fn should_auto_background(&self) -> bool {
        self.elapsed_ms() >= ASSISTANT_BLOCKING_BUDGET_MS
    }

    /// Elapsed time in milliseconds.
    pub fn elapsed_ms(&self) -> u64 {
        self.start_time.elapsed().as_millis() as u64
    }

    /// Elapsed time in seconds (for progress display).
    pub fn elapsed_seconds(&self) -> f64 {
        self.start_time.elapsed().as_secs_f64()
    }

    /// Build a progress snapshot.
    pub fn snapshot(&mut self) -> BashProgress {
        self.progress_emitted = true;
        BashProgress {
            output: self.last_output.clone(),
            elapsed_time_seconds: self.elapsed_seconds(),
            total_lines: self.total_lines,
            total_bytes: self.total_bytes,
        }
    }

    /// Whether any progress was ever emitted.
    pub fn was_progress_emitted(&self) -> bool {
        self.progress_emitted
    }
}

/// A progress snapshot for a running bash command.
#[derive(Debug, Clone)]
pub struct BashProgress {
    pub output: String,
    pub elapsed_time_seconds: f64,
    pub total_lines: i64,
    pub total_bytes: i64,
}

// ── CWD tracking ──

/// Check if a command contains any `cd` subcommands.
pub fn command_has_any_cd(command: &str) -> bool {
    split_simple(command).iter().any(|part| {
        let trimmed = part.trim();
        trimmed == "cd" || trimmed.starts_with("cd ")
    })
}

// ── Command type classification (analytics) ──

/// Get the command type for logging/analytics.
pub fn get_command_type_for_logging(command: &str) -> &'static str {
    for part in split_simple(command) {
        let base = part.split_whitespace().next().unwrap_or("");
        if let Some(matched) = COMMON_BACKGROUND_COMMANDS.get(base) {
            return matched;
        }
    }
    "other"
}

/// Extract description from the tool input, falling back to command truncation.
pub fn extract_description(command: &str, description: Option<&str>) -> String {
    if let Some(desc) = description
        && !desc.is_empty()
    {
        return desc.to_string();
    }
    // Truncate command to a reasonable summary length
    const MAX_SUMMARY_LEN: usize = 80;
    if command.len() <= MAX_SUMMARY_LEN {
        command.to_string()
    } else {
        format!("{}...", &command[..MAX_SUMMARY_LEN])
    }
}

// ── Internal helpers ──

/// Simple command splitting on `&&`, `||`, `;` delimiters.
fn split_simple(command: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '&' if chars.peek() == Some(&'&') => {
                chars.next();
                if !current.trim().is_empty() {
                    parts.push(current.trim().to_string());
                }
                current.clear();
            }
            '|' if chars.peek() == Some(&'|') => {
                chars.next();
                if !current.trim().is_empty() {
                    parts.push(current.trim().to_string());
                }
                current.clear();
            }
            ';' => {
                if !current.trim().is_empty() {
                    parts.push(current.trim().to_string());
                }
                current.clear();
            }
            _ => current.push(c),
        }
    }

    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }

    parts
}

/// Split command into parts preserving operators as separate tokens.
fn split_command_with_operators(command: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    while let Some(c) = chars.next() {
        if c == '\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            current.push(c);
            continue;
        }
        if c == '"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            current.push(c);
            continue;
        }

        if in_single_quote || in_double_quote {
            current.push(c);
            continue;
        }

        match c {
            '&' if chars.peek() == Some(&'&') => {
                chars.next();
                if !current.trim().is_empty() {
                    parts.push(current.trim().to_string());
                }
                current.clear();
                parts.push("&&".to_string());
            }
            '|' if chars.peek() == Some(&'|') => {
                chars.next();
                if !current.trim().is_empty() {
                    parts.push(current.trim().to_string());
                }
                current.clear();
                parts.push("||".to_string());
            }
            '|' => {
                if !current.trim().is_empty() {
                    parts.push(current.trim().to_string());
                }
                current.clear();
                parts.push("|".to_string());
            }
            ';' => {
                if !current.trim().is_empty() {
                    parts.push(current.trim().to_string());
                }
                current.clear();
                parts.push(";".to_string());
            }
            '>' if chars.peek() == Some(&'>') => {
                chars.next();
                if !current.trim().is_empty() {
                    parts.push(current.trim().to_string());
                }
                current.clear();
                parts.push(">>".to_string());
            }
            '>' if chars.peek() == Some(&'&') => {
                chars.next();
                if !current.trim().is_empty() {
                    parts.push(current.trim().to_string());
                }
                current.clear();
                parts.push(">&".to_string());
            }
            '>' => {
                if !current.trim().is_empty() {
                    parts.push(current.trim().to_string());
                }
                current.clear();
                parts.push(">".to_string());
            }
            _ => current.push(c),
        }
    }

    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }

    parts
}

#[cfg(test)]
#[path = "bash_advanced.test.rs"]
mod tests;
