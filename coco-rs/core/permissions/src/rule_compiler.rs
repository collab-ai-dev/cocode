//! Rule compilation — parse rule strings into structured `PermissionRule`
//! objects, match tools against compiled rule sets.

use coco_types::MCP_TOOL_PREFIX;
use coco_types::PermissionBehavior;
use coco_types::PermissionRule;
use coco_types::PermissionRuleSource;
use coco_types::PermissionRuleValue;
use coco_types::normalize_legacy_tool_name;

/// Result of matching a tool against a compiled rule set.
#[derive(Debug, Clone)]
pub struct RuleMatchResult {
    /// Whether the rule matched.
    pub matched: bool,
    /// The source of the matching rule (if any).
    pub rule_source: Option<PermissionRuleSource>,
    /// Whether the match was on content (e.g. a Bash sub-command pattern).
    pub content_match: bool,
    /// The behavior of the matching rule.
    pub behavior: Option<PermissionBehavior>,
    /// The matched rule (if any).
    pub rule: Option<PermissionRule>,
}

impl RuleMatchResult {
    fn no_match() -> Self {
        Self {
            matched: false,
            rule_source: None,
            content_match: false,
            behavior: None,
            rule: None,
        }
    }

    fn from_rule(rule: &PermissionRule, content_match: bool) -> Self {
        Self {
            matched: true,
            rule_source: Some(rule.source),
            content_match,
            behavior: Some(rule.behavior),
            rule: Some(rule.clone()),
        }
    }
}

// ── Rule string parsing ──

/// Escape special characters in rule content for storage.
///
/// Permission rules use `Tool(content)` format, so parentheses must be escaped.
/// Order: escape backslashes first, then parentheses.
pub fn escape_rule_content(content: &str) -> String {
    content
        .replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
}

/// Unescape special characters in rule content after parsing.
///
/// Reverse of `escape_rule_content`: unescape parens first, then backslashes.
pub fn unescape_rule_content(content: &str) -> String {
    content
        .replace("\\(", "(")
        .replace("\\)", ")")
        .replace("\\\\", "\\")
}

/// Find the index of the first unescaped occurrence of `ch`.
///
/// A character is escaped if preceded by an odd number of backslashes.
fn find_first_unescaped(s: &str, ch: char) -> Option<usize> {
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == ch as u8 {
            let mut backslashes = 0usize;
            let mut j = i;
            while j > 0 {
                j -= 1;
                if bytes[j] == b'\\' {
                    backslashes += 1;
                } else {
                    break;
                }
            }
            if backslashes.is_multiple_of(2) {
                return Some(i);
            }
        }
    }
    None
}

/// Find the index of the last unescaped occurrence of `ch`.
fn find_last_unescaped(s: &str, ch: char) -> Option<usize> {
    let bytes = s.as_bytes();
    for i in (0..bytes.len()).rev() {
        if bytes[i] == ch as u8 {
            let mut backslashes = 0usize;
            let mut j = i;
            while j > 0 {
                j -= 1;
                if bytes[j] == b'\\' {
                    backslashes += 1;
                } else {
                    break;
                }
            }
            if backslashes.is_multiple_of(2) {
                return Some(i);
            }
        }
    }
    None
}

/// Parse a permission rule string into a `PermissionRuleValue`.
///
/// Format: `"ToolName"` or `"ToolName(content)"`.
/// Content may contain escaped parentheses (`\(` and `\)`).
///
/// Examples:
/// - `"Bash"` → `{ tool_pattern: "Bash", rule_content: None }`
/// - `"Bash(npm install)"` → `{ tool_pattern: "Bash", rule_content: Some("npm install") }`
/// - `"Bash(python -c \"print\\(1\\)\")"` → content is unescaped
pub fn parse_rule_string(rule_string: &str) -> PermissionRuleValue {
    let open = match find_first_unescaped(rule_string, '(') {
        Some(idx) => idx,
        None => {
            return PermissionRuleValue {
                tool_pattern: normalize_legacy_tool_name(rule_string).to_string(),
                rule_content: None,
            };
        }
    };

    let close = match find_last_unescaped(rule_string, ')') {
        Some(idx) if idx > open => idx,
        _ => {
            return PermissionRuleValue {
                tool_pattern: normalize_legacy_tool_name(rule_string).to_string(),
                rule_content: None,
            };
        }
    };

    // Closing paren must be at the end
    if close != rule_string.len() - 1 {
        return PermissionRuleValue {
            tool_pattern: normalize_legacy_tool_name(rule_string).to_string(),
            rule_content: None,
        };
    }

    let tool_name = &rule_string[..open];

    // Missing tool name (e.g. "(foo)") is malformed
    if tool_name.is_empty() {
        return PermissionRuleValue {
            tool_pattern: normalize_legacy_tool_name(rule_string).to_string(),
            rule_content: None,
        };
    }

    let raw_content = &rule_string[open + 1..close];

    // Empty content or standalone wildcard → tool-wide rule
    if raw_content.is_empty() || raw_content == "*" {
        return PermissionRuleValue {
            tool_pattern: normalize_legacy_tool_name(tool_name).to_string(),
            rule_content: None,
        };
    }

    let unescaped = unescape_rule_content(raw_content);
    PermissionRuleValue {
        tool_pattern: normalize_legacy_tool_name(tool_name).to_string(),
        rule_content: Some(unescaped),
    }
}

/// Convert a `PermissionRuleValue` back to its string representation.
///
/// Escapes parentheses in content.
pub fn rule_value_to_string(value: &PermissionRuleValue) -> String {
    match &value.rule_content {
        Some(content) => {
            let escaped = escape_rule_content(content);
            format!("{}({escaped})", value.tool_pattern)
        }
        None => value.tool_pattern.clone(),
    }
}

// ── Rule compilation ──

/// Compile rule strings into structured `PermissionRule` objects.
///
/// Takes parallel arrays of `(source, behavior, rule_string)` and produces
/// structured rules with parsed tool patterns and content.
pub fn compile_rules(
    entries: &[(PermissionRuleSource, PermissionBehavior, &str)],
) -> Vec<PermissionRule> {
    entries
        .iter()
        .map(|(source, behavior, rule_str)| {
            let value = parse_rule_string(rule_str);
            PermissionRule {
                source: *source,
                behavior: *behavior,
                value,
            }
        })
        .collect()
}

// ── Rule evaluation ──

/// Check if a tool name matches a rule's tool pattern.
///
/// Supports:
/// - Exact match: `"Bash"` matches `"Bash"`
/// - Wildcard: `"*"` matches everything
/// - Prefix-wildcard: `"mcp__slack__*"` matches `"mcp__slack__send"`
/// - MCP server-level: `"mcp__server1"` matches `"mcp__server1__tool1"`
fn tool_matches_pattern(pattern: &str, tool_name: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    // Prefix wildcard: "mcp__slack__*" matches "mcp__slack__send"
    if let Some(prefix) = pattern.strip_suffix('*') {
        return tool_name.starts_with(prefix);
    }

    // Exact match
    if pattern == tool_name {
        return true;
    }

    // MCP server-level match: rule "mcp__server1" matches "mcp__server1__tool1"
    if pattern.starts_with(MCP_TOOL_PREFIX) && tool_name.starts_with(MCP_TOOL_PREFIX) {
        let rule_parts: Vec<&str> = pattern.splitn(3, "__").collect();
        let tool_parts: Vec<&str> = tool_name.splitn(3, "__").collect();
        if rule_parts.len() == 2 && tool_parts.len() == 3 && rule_parts[1] == tool_parts[1] {
            return true;
        }
    }

    false
}

/// Evaluate a set of compiled rules against a specific tool call.
///
/// Returns the first matching `RuleMatchResult`. Rules are checked in order:
/// deny rules first, then allow, then ask. Within each behavior group, rules
/// are checked in the given order (caller should pre-sort by source priority).
///
/// `tool_content` is the content to match against content-specific rules
/// (e.g. the bash command for `Bash(git *)` rules).
pub fn evaluate_rules_for_tool(
    rules: &[PermissionRule],
    tool_name: &str,
    tool_content: Option<&str>,
) -> RuleMatchResult {
    // Phase 1: Deny rules (deny always wins)
    for rule in rules
        .iter()
        .filter(|r| r.behavior == PermissionBehavior::Deny)
    {
        if let Some(result) = try_match_rule(rule, tool_name, tool_content) {
            return result;
        }
    }

    // Phase 2: Allow rules
    for rule in rules
        .iter()
        .filter(|r| r.behavior == PermissionBehavior::Allow)
    {
        if let Some(result) = try_match_rule(rule, tool_name, tool_content) {
            return result;
        }
    }

    // Phase 3: Ask rules
    for rule in rules
        .iter()
        .filter(|r| r.behavior == PermissionBehavior::Ask)
    {
        if let Some(result) = try_match_rule(rule, tool_name, tool_content) {
            return result;
        }
    }

    RuleMatchResult::no_match()
}

/// Try to match a single rule against a tool name and optional content.
fn try_match_rule(
    rule: &PermissionRule,
    tool_name: &str,
    tool_content: Option<&str>,
) -> Option<RuleMatchResult> {
    if !tool_matches_pattern(&rule.value.tool_pattern, tool_name) {
        return None;
    }

    match (&rule.value.rule_content, tool_content) {
        // Rule has content constraint and tool has content to check
        (Some(rule_content), Some(tc)) => {
            if content_matches(rule_content, tc) {
                Some(RuleMatchResult::from_rule(rule, true))
            } else {
                None // Pattern matches but content doesn't
            }
        }
        // Rule has content constraint but tool has no content → skip
        (Some(_), None) => None,
        // Rule has no content constraint → tool-level match
        (None, _) => Some(RuleMatchResult::from_rule(rule, false)),
    }
}

/// Check if tool content matches a rule's content pattern.
///
/// Supports prefix matching (ending with `*`) and exact matching.
fn content_matches(rule_content: &str, tool_content: &str) -> bool {
    if rule_content == "*" {
        return true;
    }

    // Prefix with wildcard: "git *" matches "git status", "git push", etc.
    if let Some(prefix) = rule_content.strip_suffix('*') {
        return tool_content.starts_with(prefix);
    }

    // Exact match
    rule_content == tool_content
}

#[cfg(test)]
#[path = "rule_compiler.test.rs"]
mod tests;
