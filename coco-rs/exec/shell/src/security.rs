//! Shell command security checks.
//!
//! Runs a battery of checks against command strings to detect injection,
//! obfuscation, and dangerous patterns before execution.
//!
//! The bulk of the breadth comes from the quote/heredoc-aware analyzer suite
//! in `coco_shell_parser::security` (`default_analyzers()` + `analyze()`).
//! Those 29 analyzers run against a tree-sitter parse and surface every risk
//! the validator suite catches. Every analyzer-caught risk maps to
//! [`SecuritySeverity::Ask`] — routed through the normal permission prompt,
//! never an outright deny.
//!
//! A small set of additional checks (`Deny` for raw control characters /
//! `/proc/*/environ` access) is a DELIBERATE coco-rs divergence: those are
//! genuinely catastrophic, near-always-malicious constructs that we block
//! without prompting.

use crate::safety::SecurityCheckId;
use coco_shell_parser::ShellParser;
use coco_shell_parser::security;
use coco_shell_parser::security::RiskKind;

/// Result of a single security check.
#[derive(Debug, Clone)]
pub struct SecurityCheck {
    pub id: SecurityCheckId,
    pub severity: SecuritySeverity,
    pub message: String,
}

/// Severity level for a security check result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecuritySeverity {
    /// Command must be denied outright.
    Deny,
    /// Command needs user confirmation.
    Ask,
}

/// Run all security checks on a command string.
///
/// Combines the full `coco_shell_parser` analyzer suite (mapped to `Ask`) with
/// a couple of coco-rs-specific catastrophic `Deny` checks (raw control
/// characters, `/proc/*/environ` access).
pub fn check_security(command: &str) -> Vec<SecurityCheck> {
    let mut checks = analyzer_checks(command);

    // coco-rs-specific Deny checks that the analyzer suite does not cover.
    let deny_checks: [fn(&str) -> Option<SecurityCheck>; 2] =
        [check_control_characters, check_proc_environ];
    checks.extend(deny_checks.iter().filter_map(|check| check(command)));

    checks
}

/// Run the `coco_shell_parser` analyzer suite and map each risk to an `Ask`
/// security check. Every pattern routes through the permission prompt, never
/// an outright deny.
fn analyzer_checks(command: &str) -> Vec<SecurityCheck> {
    let mut parser = ShellParser::new();
    let parsed = parser.parse(command);
    let analysis = security::analyze(&parsed);

    analysis
        .risks
        .iter()
        .map(|risk| SecurityCheck {
            id: risk_check_id(risk.kind),
            severity: SecuritySeverity::Ask,
            message: risk.message.clone(),
        })
        .collect()
}

/// Map an analyzer [`RiskKind`] onto the closest [`SecurityCheckId`].
fn risk_check_id(kind: RiskKind) -> SecurityCheckId {
    match kind {
        RiskKind::SingleQuoteBypass => SecurityCheckId::MALFORMED_TOKEN_INJECTION,
        RiskKind::JqDanger => SecurityCheckId::JQ_SYSTEM_FUNCTION,
        RiskKind::ObfuscatedFlags => SecurityCheckId::OBFUSCATED_FLAGS,
        RiskKind::ShellMetacharacters => SecurityCheckId::SHELL_METACHARACTERS,
        RiskKind::DangerousVariables => SecurityCheckId::DANGEROUS_VARIABLES,
        RiskKind::NewlineInjection => SecurityCheckId::NEWLINES,
        RiskKind::IfsInjection => SecurityCheckId::IFS_INJECTION,
        RiskKind::ProcEnvironAccess => SecurityCheckId::PROC_ENVIRON_ACCESS,
        RiskKind::BackslashEscapedWhitespace => SecurityCheckId::BACKSLASH_ESCAPED_WHITESPACE,
        RiskKind::BackslashEscapedOperators => SecurityCheckId::BACKSLASH_ESCAPED_OPERATORS,
        RiskKind::UnicodeWhitespace => SecurityCheckId::UNICODE_WHITESPACE,
        RiskKind::MidWordHash => SecurityCheckId::MID_WORD_HASH,
        RiskKind::BraceExpansion => SecurityCheckId::BRACE_EXPANSION,
        RiskKind::ZshDangerousCommands => SecurityCheckId::ZSH_DANGEROUS_COMMANDS,
        RiskKind::CommentQuoteDesync => SecurityCheckId::COMMENT_QUOTE_DESYNC,
        RiskKind::QuotedNewlineHash => SecurityCheckId::QUOTED_NEWLINE,
        RiskKind::ExcessClosingBraces => SecurityCheckId::MALFORMED_TOKEN_INJECTION,
        RiskKind::EvalLikeBuiltin => SecurityCheckId::DANGEROUS_PATTERNS_GENERAL,
        RiskKind::SubscriptEval => SecurityCheckId::DANGEROUS_PATTERNS_GENERAL,
        RiskKind::ArithComparison => SecurityCheckId::DANGEROUS_PATTERNS_CONDITIONAL,
        RiskKind::UnsafeHeredocSubstitution => SecurityCheckId::DANGEROUS_PATTERNS_SUBSHELL,
        RiskKind::DangerousSubstitution => SecurityCheckId::DANGEROUS_PATTERNS_SUBSHELL,
        RiskKind::MalformedTokens => SecurityCheckId::MALFORMED_TOKEN_INJECTION,
        RiskKind::SensitiveRedirect => SecurityCheckId::DANGEROUS_PATTERNS_REDIRECTION,
        RiskKind::NetworkExfiltration => SecurityCheckId::DANGEROUS_PATTERNS_GENERAL,
        RiskKind::PrivilegeEscalation => SecurityCheckId::DANGEROUS_PATTERNS_GENERAL,
        RiskKind::FileSystemTampering => SecurityCheckId::DANGEROUS_PATTERNS_GENERAL,
        RiskKind::CodeExecution => SecurityCheckId::DANGEROUS_PATTERNS_GENERAL,
        RiskKind::DangerousPath => SecurityCheckId::DANGEROUS_PATTERNS_GENERAL,
    }
}

/// Check for Unicode control characters and zero-width characters.
///
/// Catastrophic-by-design `Deny`: raw control bytes in a command are almost
/// always an obfuscation / injection attempt and have no legitimate use here.
fn check_control_characters(command: &str) -> Option<SecurityCheck> {
    for ch in command.chars() {
        // Zero-width chars
        if matches!(ch, '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}') {
            return Some(SecurityCheck {
                id: SecurityCheckId::CONTROL_CHARACTERS,
                severity: SecuritySeverity::Deny,
                message: format!("zero-width character U+{:04X} detected", ch as u32),
            });
        }
        // Unicode control characters (excluding common whitespace)
        if ch.is_control() && !matches!(ch, '\n' | '\r' | '\t') {
            return Some(SecurityCheck {
                id: SecurityCheckId::CONTROL_CHARACTERS,
                severity: SecuritySeverity::Deny,
                message: format!("control character U+{:04X} detected", ch as u32),
            });
        }
    }
    None
}

/// Check for /proc/*/environ access.
///
/// Catastrophic-by-design `Deny`: reading another process's environment leaks
/// secrets and has no legitimate place in agent-issued commands.
fn check_proc_environ(command: &str) -> Option<SecurityCheck> {
    if command.contains("/proc/") && command.contains("/environ") {
        return Some(SecurityCheck {
            id: SecurityCheckId::PROC_ENVIRON_ACCESS,
            severity: SecuritySeverity::Deny,
            message: "/proc/*/environ access detected".into(),
        });
    }
    None
}

#[cfg(test)]
#[path = "security.test.rs"]
mod tests;
