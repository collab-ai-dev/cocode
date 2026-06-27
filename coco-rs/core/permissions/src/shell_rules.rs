//! Shell permission rule parsing and matching.
//!
//! Three rule types for permission matching:
//! - Exact: "git commit" matches only "git commit"
//! - Prefix: "git " or "git:*" matches "git commit", "git push", etc.
//! - Wildcard: "git *" matches any git subcommand (with escape support)

use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::Mutex;

/// Case sensitivity for shell-rule command matching. Bash matches
/// case-sensitively; PowerShell case-insensitively.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShellCase {
    Sensitive,
    Insensitive,
}

/// Case-aware string equality.
fn str_eq(a: &str, b: &str, case: ShellCase) -> bool {
    match case {
        ShellCase::Sensitive => a == b,
        ShellCase::Insensitive => a.to_lowercase() == b.to_lowercase(),
    }
}

/// Case-aware prefix test.
fn str_starts_with(s: &str, prefix: &str, case: ShellCase) -> bool {
    match case {
        ShellCase::Sensitive => s.starts_with(prefix),
        ShellCase::Insensitive => s.to_lowercase().starts_with(&prefix.to_lowercase()),
    }
}

/// A parsed shell permission rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ShellPermissionRule {
    /// Exact command match.
    Exact { command: String },
    /// Prefix match (command starts with this string).
    Prefix { prefix: String },
    /// Wildcard match using glob-like patterns with escape support.
    Wildcard { pattern: String },
}

/// Extract prefix from legacy `:*` syntax (e.g., "npm:*" → "npm").
fn extract_legacy_prefix(rule: &str) -> Option<&str> {
    rule.strip_suffix(":*")
}

/// Check if a pattern contains unescaped wildcards (not legacy `:*` syntax).
///
/// A `*` is unescaped if preceded by an even number of backslashes (including 0).
fn has_wildcards(pattern: &str) -> bool {
    // Legacy :* is not a wildcard
    if pattern.ends_with(":*") {
        return false;
    }

    let bytes = pattern.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'*' {
            let mut backslash_count = 0;
            let mut j = i;
            while j > 0 {
                j -= 1;
                if bytes[j] == b'\\' {
                    backslash_count += 1;
                } else {
                    break;
                }
            }
            // Even number of backslashes → unescaped star
            if backslash_count % 2 == 0 {
                return true;
            }
        }
    }
    false
}

impl ShellPermissionRule {
    /// Parse a rule string into a ShellPermissionRule.
    ///
    /// Rules:
    /// - Ends with `:*` → Prefix (legacy syntax, e.g. "npm:*" → prefix "npm")
    /// - Contains unescaped `*` → Wildcard
    /// - Ends with space → Prefix
    /// - Otherwise → Exact
    pub fn parse(rule: &str) -> Self {
        // Legacy :* prefix syntax first (backwards compatibility)
        if let Some(prefix) = extract_legacy_prefix(rule) {
            return Self::Prefix {
                prefix: prefix.to_string(),
            };
        }

        // New wildcard syntax (contains unescaped *)
        if has_wildcards(rule) {
            return Self::Wildcard {
                pattern: rule.to_string(),
            };
        }

        // Trailing space → prefix
        if rule.ends_with(' ') {
            return Self::Prefix {
                prefix: rule.to_string(),
            };
        }

        Self::Exact {
            command: rule.to_string(),
        }
    }

    /// Check if a command matches this rule (case-sensitive — Bash posture).
    pub fn matches(&self, command: &str) -> bool {
        self.matches_cased(command, ShellCase::Sensitive)
    }

    /// Check if a command matches this rule under the given case sensitivity.
    /// PowerShell uses [`ShellCase::Insensitive`].
    pub fn matches_cased(&self, command: &str, case: ShellCase) -> bool {
        match self {
            Self::Exact { command: expected } => str_eq(expected, command, case),
            Self::Prefix { prefix } => str_starts_with(command, prefix.as_str(), case),
            Self::Wildcard { pattern } => match_wildcard_pattern_cased(pattern, command, case),
        }
    }
}

/// Process-wide cache of compiled wildcard regexes keyed by raw rule pattern.
///
/// Permission checks run on the hot path (every bash invocation); recompiling
/// the regex each time adds up. `None` is cached for patterns that fail to
/// compile so we don't retry.
///
/// Bounded at `WILDCARD_REGEX_CACHE_MAX` entries. Once full, new patterns are
/// compiled on-demand but NOT cached — this caps memory at O(cap × avg regex
/// size) even if a compromised plugin feeds unbounded unique patterns. In
/// practice rule sets are small (~dozens), so the cap is rarely hit.
/// Compiled-wildcard cache, keyed by `(pattern, case)` so a case-sensitive
/// compile never poisons a case-insensitive lookup (and vice-versa).
type WildcardRegexCache = Mutex<HashMap<(String, ShellCase), Option<regex::Regex>>>;

static WILDCARD_REGEX_CACHE: LazyLock<WildcardRegexCache> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

const WILDCARD_REGEX_CACHE_MAX: usize = 1024;

/// Match a command against a wildcard pattern with escape support.
///
/// - `*` matches any sequence of characters (including newlines)
/// - `\*` matches a literal `*`
/// - `\\` matches a literal `\`
/// - Trailing ` *` (space + single wildcard) is optional — `git *` matches bare `git`
///
/// Case-sensitive convenience wrapper (Bash posture) used by the wildcard
/// unit tests; production code calls [`match_wildcard_pattern_cased`] directly.
#[cfg(test)]
fn match_wildcard_pattern(pattern: &str, command: &str) -> bool {
    match_wildcard_pattern_cased(pattern, command, ShellCase::Sensitive)
}

/// Case-aware wildcard match. `Insensitive` compiles the regex with the `(?i)` flag.
fn match_wildcard_pattern_cased(pattern: &str, command: &str, case: ShellCase) -> bool {
    // PoisonError handling: if a thread panicked while holding the cache lock,
    // the cache contents are still consistent (we only store Option<Regex> and
    // never break invariants mid-write). Recover via `into_inner()`.
    let key = (pattern.to_string(), case);
    {
        let cache = WILDCARD_REGEX_CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = cache.get(&key) {
            return entry.as_ref().is_some_and(|re| re.is_match(command));
        }
    }

    let compiled = compile_wildcard_regex(pattern, case);
    let result = compiled.as_ref().is_some_and(|re| re.is_match(command));
    let mut cache = WILDCARD_REGEX_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if cache.len() < WILDCARD_REGEX_CACHE_MAX {
        cache.insert(key, compiled);
    }
    // Cache full: skip insertion — the pattern will be recompiled on each call,
    // which is acceptable degradation compared to unbounded memory growth.
    result
}

/// Build a regex from a wildcard rule pattern. Returns `None` if compilation
/// fails (invalid pattern — we log once and treat as no-match).
fn compile_wildcard_regex(pattern: &str, case: ShellCase) -> Option<regex::Regex> {
    let trimmed = pattern.trim();

    // Phase 1: Process escape sequences, collecting regex-ready segments
    // Use sentinel markers for escaped chars to avoid interference with regex escaping
    const ESCAPED_STAR: &str = "\x00ES\x00";
    const ESCAPED_BACKSLASH: &str = "\x00EB\x00";

    let mut processed = String::with_capacity(trimmed.len());
    let bytes = trimmed.as_bytes();
    let mut i = 0;
    let mut unescaped_star_count = 0;

    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'*' => {
                    processed.push_str(ESCAPED_STAR);
                    i += 2;
                    continue;
                }
                b'\\' => {
                    processed.push_str(ESCAPED_BACKSLASH);
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }
        if bytes[i] == b'*' {
            unescaped_star_count += 1;
        }
        processed.push(bytes[i] as char);
        i += 1;
    }

    // Phase 2: Escape regex special chars (except *)
    let escaped = regex_escape_except_star(&processed);

    // Phase 3: Convert unescaped * to .*
    let with_wildcards = escaped.replace('*', ".*");

    // Phase 4: Restore escaped literals
    let mut regex_pattern = with_wildcards
        .replace(ESCAPED_STAR, r"\*")
        .replace(ESCAPED_BACKSLASH, r"\\");

    // Phase 5: Trailing ` *` with single wildcard → optional
    // "git *" matches both "git add" and bare "git"
    if regex_pattern.ends_with(" .*") && unescaped_star_count == 1 {
        let len = regex_pattern.len();
        regex_pattern.replace_range(len - 3.., "( .*)?");
    }

    // Phase 6: Match entire string with dotAll semantics (+ case-insensitive
    // for PowerShell).
    let flags = match case {
        ShellCase::Sensitive => "(?s)",
        ShellCase::Insensitive => "(?si)",
    };
    let full_pattern = format!("{flags}^{regex_pattern}$");
    match regex::Regex::new(&full_pattern) {
        Ok(re) => Some(re),
        Err(e) => {
            tracing::warn!(
                pattern = %pattern,
                "invalid wildcard pattern, regex compilation failed: {e}"
            );
            None
        }
    }
}

/// Escape regex special characters except `*`.
fn regex_escape_except_star(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        match ch {
            '.' | '+' | '?' | '^' | '$' | '{' | '}' | '(' | ')' | '|' | '[' | ']' | '\\' | '\''
            | '"' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

/// Posture for matching a Bash rule against a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleMatchPolicy {
    /// Deny or Ask: strip ALL leading env vars to a fixed point and re-split
    /// into subcommands so a denied/asked command cannot be hidden behind
    /// wrappers, env prefixes, or compounds (`FOO=1 curl`, `timeout 5 curl`,
    /// `echo hi && curl`).
    DenyOrAsk,
    /// Allow: safe-wrapper/redirection stripping only; prefix/wildcard rules
    /// MUST NOT match a compound candidate (cannot widen an allow by chaining,
    /// e.g. `Bash(cd:*)` must not auto-allow `cd /p && curl evil`).
    Allow,
}

/// Build the candidate-command set a rule is tested against.
///
/// Produces: the original command (quotes preserved, for exact rules), the
/// redirection-stripped form, safe-wrapper-stripped forms, and — for deny/ask
/// — the env-var fixed-point expansion.
fn build_candidate_commands(command: &str, policy: RuleMatchPolicy) -> Vec<String> {
    let command = command.trim().to_string();
    let without_redir = coco_shell::strip_output_redirections(&command);
    let base: Vec<String> = if without_redir != command {
        vec![command, without_redir]
    } else {
        vec![command]
    };

    let mut candidates: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let push =
        |v: String, candidates: &mut Vec<String>, seen: &mut std::collections::HashSet<String>| {
            if seen.insert(v.clone()) {
                candidates.push(v);
            }
        };
    for c in &base {
        push(c.clone(), &mut candidates, &mut seen);
        let w = coco_shell::strip_safe_wrappers(c);
        push(w, &mut candidates, &mut seen);
    }

    // Deny/ask: fixed-point env-var + wrapper expansion so a deny rule cannot be
    // bypassed by a leading `FOO=1 ` / `timeout 5 ` prefix.
    if policy == RuleMatchPolicy::DenyOrAsk {
        let mut start = 0;
        while start < candidates.len() {
            let end = candidates.len();
            for i in start..end {
                let cmd = candidates[i].clone();
                let env_stripped =
                    coco_shell::strip_all_env_vars(&cmd, /*check_hijack*/ false);
                push(env_stripped, &mut candidates, &mut seen);
                let wrapper_stripped = coco_shell::strip_safe_wrappers(&cmd);
                push(wrapper_stripped, &mut candidates, &mut seen);
            }
            start = end;
        }
    }
    candidates
}

/// Whether `rule` matches `candidate` with the allow-posture compound guard:
/// a prefix/wildcard rule never matches a compound command (so chaining cannot
/// widen an allow).
fn rule_matches_candidate_with_compound_guard(
    rule: &ShellPermissionRule,
    candidate: &str,
    case: ShellCase,
) -> bool {
    match rule {
        ShellPermissionRule::Exact { .. } => rule.matches_cased(candidate, case),
        ShellPermissionRule::Prefix { prefix } => {
            if coco_shell::split_compound_command(candidate).len() > 1 {
                return false;
            }
            // The prefix is the bare word; normalize legacy/literal
            // trailing-space forms and also allow an `xargs <prefix>` wrapper.
            let bare = prefix.trim_end();
            str_eq(candidate, bare, case)
                || str_starts_with(candidate, &format!("{bare} "), case)
                || str_eq(candidate, &format!("xargs {bare}"), case)
                || str_starts_with(candidate, &format!("xargs {bare} "), case)
        }
        ShellPermissionRule::Wildcard { pattern } => {
            if coco_shell::split_compound_command(candidate).len() > 1 {
                return false;
            }
            match_wildcard_pattern_cased(pattern, candidate, case)
        }
    }
}

/// Check if a shell `rule_content` matches `command` under the given posture
/// and case sensitivity. The entry point for content-specific permission
/// matching (deny / allow / ask). `case` is [`ShellCase::Sensitive`] for Bash
/// and [`ShellCase::Insensitive`] for PowerShell. Replaces the old
/// whole-command `matches_bash_rule` so a deny rule can no longer be bypassed
/// by wrapping, env prefixes, or command chaining.
pub fn match_bash_rule(
    rule_content: &str,
    command: &str,
    policy: RuleMatchPolicy,
    case: ShellCase,
) -> bool {
    let rule = ShellPermissionRule::parse(rule_content);
    let candidates = build_candidate_commands(command, policy);
    match policy {
        RuleMatchPolicy::Allow => candidates
            .iter()
            .any(|c| rule_matches_candidate_with_compound_guard(&rule, c, case)),
        RuleMatchPolicy::DenyOrAsk => {
            // (a) skipCompoundCheck: raw match against each candidate.
            if candidates.iter().any(|c| rule.matches_cased(c, case)) {
                return true;
            }
            // (b) per-segment: a denied/asked command can't hide inside a compound.
            for c in &candidates {
                for seg in coco_shell::split_compound_command(c) {
                    if rule.matches_cased(&seg, case)
                        || rule_matches_candidate_with_compound_guard(&rule, &seg, case)
                    {
                        return true;
                    }
                }
            }
            false
        }
    }
}

/// Check if a rule_content represents a dangerous bash permission.
///
/// Delegates to `setup::is_dangerous_bash_permission` for the full pattern list.
pub fn is_dangerous_bash_permission(rule_content: &str) -> bool {
    use coco_types::ToolName;
    crate::setup::is_dangerous_bash_permission(
        ToolName::Bash.as_str(),
        Some(rule_content),
        /*is_ant_user*/ false,
    )
}

/// Generate a permission update suggestion for an exact command.
pub fn suggestion_for_exact_command(
    tool_name: &str,
    command: &str,
) -> coco_types::PermissionUpdate {
    coco_types::PermissionUpdate::AddRules {
        rules: vec![coco_types::PermissionRule {
            source: coco_types::PermissionRuleSource::LocalSettings,
            behavior: coco_types::PermissionBehavior::Allow,
            value: coco_types::PermissionRuleValue {
                tool_pattern: tool_name.to_string(),
                rule_content: Some(command.to_string()),
            },
        }],
        destination: coco_types::PermissionUpdateDestination::LocalSettings,
    }
}

/// Generate a permission update suggestion for a prefix match.
pub fn suggestion_for_prefix(tool_name: &str, prefix: &str) -> coco_types::PermissionUpdate {
    coco_types::PermissionUpdate::AddRules {
        rules: vec![coco_types::PermissionRule {
            source: coco_types::PermissionRuleSource::LocalSettings,
            behavior: coco_types::PermissionBehavior::Allow,
            value: coco_types::PermissionRuleValue {
                tool_pattern: tool_name.to_string(),
                rule_content: Some(format!("{prefix}:*")),
            },
        }],
        destination: coco_types::PermissionUpdateDestination::LocalSettings,
    }
}

/// Build the "always allow" suggestions offered alongside a shell approval
/// prompt — the `Bash(git status:*)` row a user can accept to stop being asked
/// for that command family.
///
/// A heredoc or other multiline command keys on a stable prefix — an exact rule
/// would never re-match (the body changes every call) and a multiline body can
/// embed `:*` mid-pattern, corrupting the settings file. A single-line command
/// keys on its `command subcommand` prefix when one is extractable
/// ([`coco_shell::get_command_prefix`]), otherwise on the exact command.
///
/// `tool_name` is the shell tool the rule targets (`Bash` / `PowerShell`).
/// Returns an empty vec for an empty command.
pub fn bash_permission_suggestions(
    tool_name: &str,
    command: &str,
) -> Vec<coco_types::PermissionUpdate> {
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "/".to_string());
    bash_permission_suggestions_in_cwd(tool_name, command, &cwd)
}

pub fn bash_permission_suggestions_in_cwd(
    tool_name: &str,
    command: &str,
    cwd: &str,
) -> Vec<coco_types::PermissionUpdate> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let significant = significant_bash_subcommands_in_cwd(trimmed, cwd);
    if significant.len() > 1 {
        return suggestions_for_significant_subcommands(tool_name, &significant);
    }
    if let Some(only) = significant.first()
        && only != trimmed
    {
        return bash_permission_suggestions_for_single(tool_name, only);
    }

    bash_permission_suggestions_for_single(tool_name, trimmed)
}

fn bash_permission_suggestions_for_single(
    tool_name: &str,
    command: &str,
) -> Vec<coco_types::PermissionUpdate> {
    let trimmed = command.trim();

    // Heredoc: suggest a prefix taken from the words before `<<`.
    if let Some(prefix) = coco_shell::heredoc_command_prefix(trimmed) {
        return vec![suggestion_for_prefix(tool_name, &prefix)];
    }

    // Other multiline commands make poor exact rules — key on the first line.
    if let Some((first_line, _)) = trimmed.split_once('\n') {
        let first_line = first_line.trim();
        if !first_line.is_empty() {
            return vec![suggestion_for_prefix(tool_name, first_line)];
        }
    }

    // Single line: a `command subcommand` prefix gives a reusable rule.
    if let Some(prefix) = coco_shell::get_command_prefix(trimmed) {
        return vec![suggestion_for_prefix(tool_name, &prefix)];
    }

    vec![suggestion_for_exact_command(tool_name, trimmed)]
}

fn significant_bash_subcommands(command: &str) -> Vec<String> {
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "/".to_string());
    significant_bash_subcommands_in_cwd(command, &cwd)
}

fn significant_bash_subcommands_in_cwd(command: &str, cwd: &str) -> Vec<String> {
    let analysis = coco_shell::analyze_compound_command(command, cwd);
    analysis.significant_subcommands
}

fn suggestions_for_significant_subcommands(
    tool_name: &str,
    subcommands: &[String],
) -> Vec<coco_types::PermissionUpdate> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for subcommand in subcommands {
        for suggestion in bash_permission_suggestions_for_single(tool_name, subcommand) {
            if let Some(content) = single_rule_content(&suggestion)
                && seen.insert(content.to_string())
            {
                out.push(suggestion);
            }
        }
    }
    out
}

fn single_rule_content(update: &coco_types::PermissionUpdate) -> Option<&str> {
    match update {
        coco_types::PermissionUpdate::AddRules { rules, .. } if rules.len() == 1 => {
            rules[0].value.rule_content.as_deref()
        }
        coco_types::PermissionUpdate::ReplaceRules { .. }
        | coco_types::PermissionUpdate::AddRules { .. }
        | coco_types::PermissionUpdate::RemoveRules { .. }
        | coco_types::PermissionUpdate::SetMode { .. }
        | coco_types::PermissionUpdate::AddDirectories { .. }
        | coco_types::PermissionUpdate::RemoveDirectories { .. } => None,
    }
}

/// Default text for the permission dialog's editable "always allow" prefix
/// field, given the raw bash command.
///
/// Tries a `command subcommand` prefix (`git status:*`), then a single-word
/// prefix (`ls:*`, bare shells excluded), then falls back to the exact command.
/// Unlike [`bash_permission_suggestions`] (which keys the saved rule and prefers
/// an exact rule when no clean two-word prefix exists), this seeds an *editable*
/// field, so it offers the broader single-word `:*` form via
/// [`coco_shell::get_first_word_prefix`] as a starting point the user can refine.
pub fn editable_prefix_default(command: &str) -> String {
    let trimmed = command.trim();
    let significant = significant_bash_subcommands(trimmed);
    if significant.len() == 1
        && let Some(only) = significant.first()
        && only != trimmed
    {
        return editable_prefix_default_for_single(only);
    }
    editable_prefix_default_for_single(trimmed)
}

fn editable_prefix_default_for_single(command: &str) -> String {
    let trimmed = command.trim();
    if let Some(prefix) = coco_shell::get_command_prefix(trimmed) {
        return format!("{prefix}:*");
    }
    if let Some(word) = coco_shell::get_first_word_prefix(trimmed) {
        return format!("{word}:*");
    }
    trimmed.to_string()
}

/// TUI seed value for the editable prefix field. When backend suggestions
/// contain exactly one rule, seed from that rule. When they contain multiple
/// rules, suppress the editable field so the UI saves them atomically.
pub fn editable_prefix_from_suggestions_or_command(
    command: &str,
    suggestions: &[coco_types::PermissionUpdate],
) -> Option<String> {
    let mut contents = suggestions
        .iter()
        .flat_map(permission_update_rule_contents)
        .collect::<Vec<_>>();
    contents.sort_unstable();
    contents.dedup();
    match contents.as_slice() {
        [only] => Some((*only).to_string()),
        [] => {
            if significant_bash_subcommands(command).len() > 1 {
                None
            } else {
                Some(editable_prefix_default(command))
            }
        }
        _ => None,
    }
}

fn permission_update_rule_contents(
    update: &coco_types::PermissionUpdate,
) -> impl Iterator<Item = &str> {
    match update {
        coco_types::PermissionUpdate::AddRules { rules, .. } => rules
            .iter()
            .filter_map(|rule| rule.value.rule_content.as_deref())
            .collect::<Vec<_>>()
            .into_iter(),
        coco_types::PermissionUpdate::ReplaceRules { .. }
        | coco_types::PermissionUpdate::RemoveRules { .. }
        | coco_types::PermissionUpdate::SetMode { .. }
        | coco_types::PermissionUpdate::AddDirectories { .. }
        | coco_types::PermissionUpdate::RemoveDirectories { .. } => Vec::new().into_iter(),
    }
}

#[cfg(test)]
#[path = "shell_rules.test.rs"]
mod tests;
