//! Security analyzers for detecting specific risk patterns.

use once_cell::sync::Lazy;
use regex::Regex;

use crate::parser::ParsedShell;
use crate::redirects::extract_redirects_from_tree;
use crate::segments::extract_segments_from_tree;
use crate::tokenizer::TokenKind;

use super::risks::RiskKind;
use super::risks::RiskLevel;
use super::risks::SecurityAnalysis;
use super::risks::SecurityRisk;

/// Trait for security analyzers.
pub trait Analyzer {
    /// Analyze a parsed command and add any detected risks to the analysis.
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis);
}

/// Extracts (byte_index, char) pairs for characters in unquoted context.
///
/// Tracks single-quote, double-quote, and backslash-escape state,
/// yielding only characters that appear outside any quoting construct.
fn extract_unquoted_chars(source: &str) -> Vec<(usize, char)> {
    let mut result = Vec::new();
    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    while i < len {
        let ch = bytes[i];
        if !in_single_quote && !in_double_quote && ch == b'\\' {
            i = (i + 2).min(len); // skip escaped char, clamp to bounds
            continue;
        }
        if ch == b'\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            i += 1;
            continue;
        }
        if ch == b'"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            i += 1;
            continue;
        }
        if !in_single_quote && !in_double_quote {
            result.push((i, ch as char));
        }
        i += 1;
    }
    result
}

// =============================================================================
// Layer 0: Pre-check Analyzer (highest priority)
// =============================================================================

/// Detects single-quote bypass via backslash at end of single-quoted string.
///
/// A pattern like `'test\'` has an odd number of backslashes before the closing
/// quote. A naive parser that interprets `\'` as an escape (bash doesn't support
/// escapes in single quotes) would think the quote is still open, allowing
/// injection of commands after the closing quote.
pub struct SingleQuoteBypassAnalyzer;

impl Analyzer for SingleQuoteBypassAnalyzer {
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis) {
        let source = cmd.source();
        let bytes = source.as_bytes();
        let len = bytes.len();
        let mut i = 0;

        while i < len {
            if bytes[i] != b'\'' {
                i += 1;
                continue;
            }
            // Found opening single quote
            i += 1;
            let content_start = i;
            // Scan to closing quote
            while i < len && bytes[i] != b'\'' {
                i += 1;
            }
            if i >= len {
                break; // unclosed quote
            }
            // Count backslashes immediately before closing quote
            let mut bs_count = 0usize;
            let mut j = i;
            while j > content_start && bytes[j - 1] == b'\\' {
                bs_count += 1;
                j -= 1;
            }
            // Odd number: a naive parser would interpret \' as escape
            if bs_count % 2 == 1 {
                analysis.add_risk(
                    SecurityRisk::new(
                        RiskKind::SingleQuoteBypass,
                        "backslash at end of single-quoted string may bypass quote tracking",
                    )
                    .with_matched_text(source),
                );
                return;
            }
            i += 1; // skip closing quote
        }
    }
}

// =============================================================================
// Deny Phase Analyzers
// =============================================================================

/// Detects dangerous jq operations (system() calls).
pub struct JqDangerAnalyzer;

impl Analyzer for JqDangerAnalyzer {
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis) {
        #[allow(clippy::expect_used)]
        static JQ_SYSTEM_RE: Lazy<Regex> =
            Lazy::new(|| Regex::new(r#"\bsystem\s*\("#).expect("valid regex"));

        let commands = cmd.extract_commands();
        for args in &commands {
            // Check if this is a jq command
            if args.first().is_some_and(|c| c == "jq") {
                // Check arguments for system() calls
                for arg in args.iter().skip(1) {
                    if JQ_SYSTEM_RE.is_match(arg) {
                        analysis.add_risk(
                            SecurityRisk::new(
                                RiskKind::JqDanger,
                                "jq command contains system() call which can execute arbitrary commands",
                            )
                            .with_matched_text(arg),
                        );
                    }
                }

                // Check for dangerous file-access flags
                let has_file_flag = args.iter().skip(1).any(|a| {
                    matches!(
                        a.as_str(),
                        "-f" | "--fromfile"
                            | "--rawfile"
                            | "--slurpfile"
                            | "-L"
                            | "--library-path"
                            | "--from-file"
                    )
                });
                if has_file_flag {
                    analysis.add_risk(SecurityRisk::new(
                        RiskKind::JqDanger,
                        "jq file flag can read arbitrary files",
                    ));
                }
            }
        }
    }
}

/// Detects obfuscated flags using $'...' or $"..." syntax.
pub struct ObfuscatedFlagsAnalyzer;

impl Analyzer for ObfuscatedFlagsAnalyzer {
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis) {
        for token in cmd.tokens() {
            match token.kind {
                TokenKind::AnsiCQuoted => {
                    analysis.add_risk(
                        SecurityRisk::new(
                            RiskKind::ObfuscatedFlags,
                            "ANSI-C quoting ($'...') can hide shell escape sequences",
                        )
                        .with_span(token.span)
                        .with_matched_text(&token.text),
                    );
                }
                TokenKind::LocalizedString => {
                    analysis.add_risk(
                        SecurityRisk::new(
                            RiskKind::ObfuscatedFlags,
                            "localized string ($\"...\") may contain hidden expansions",
                        )
                        .with_span(token.span)
                        .with_matched_text(&token.text),
                    );
                }
                _ => {}
            }
        }
    }
}

/// Detects shell metacharacters in command arguments.
pub struct ShellMetacharactersAnalyzer;

impl Analyzer for ShellMetacharactersAnalyzer {
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis) {
        #[allow(clippy::expect_used)]
        static DANGEROUS_METACHAR_RE: Lazy<Regex> = Lazy::new(|| {
            // Look for semicolons, pipes, or ampersands that might be injection
            Regex::new(r#"[;|&]"#).expect("valid regex")
        });

        // Check for dangerous patterns in find/grep -exec or similar
        let commands = cmd.extract_commands();
        for args in &commands {
            let cmd_name = args.first().map(String::as_str).unwrap_or("");
            if matches!(cmd_name, "find" | "xargs") {
                // Check for -exec or similar flags with embedded metacharacters
                for (i, arg) in args.iter().enumerate() {
                    if (arg == "-exec" || arg == "-execdir" || arg == "-ok") && i + 1 < args.len() {
                        // Check the command being executed
                        for exec_arg in &args[i + 1..] {
                            // Stop at -exec terminators (both escaped and unescaped forms)
                            if matches!(exec_arg.as_str(), ";" | "+" | "\\;" | "\\+") {
                                break;
                            }
                            if DANGEROUS_METACHAR_RE.is_match(exec_arg) {
                                analysis.add_risk(
                                    SecurityRisk::new(
                                        RiskKind::ShellMetacharacters,
                                        format!("shell metacharacter in {cmd_name} -exec argument may allow command injection"),
                                    )
                                    .with_matched_text(exec_arg),
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Detects dangerous variable patterns.
pub struct DangerousVariablesAnalyzer;

impl Analyzer for DangerousVariablesAnalyzer {
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis) {
        // Look for `$VAR`/`${VAR}` adjacent to a pipe OR a redirect, in unquoted
        // context only. Inside double quotes, `"$VAR" | cmd` is safe because the
        // variable expands as a single word. Flags both orderings: `$VAR\s*[|<>]`
        // and `[<>|]\s*$VAR`.
        #[allow(clippy::expect_used)]
        static VAR_REDIR_RE: Lazy<Regex> = Lazy::new(|| {
            Regex::new(
                r#"(\$\{?[A-Za-z_][A-Za-z0-9_]*\}?\s*[|<>])|([|<>]\s*\$\{?[A-Za-z_][A-Za-z0-9_]*\}?)"#,
            )
            .expect("valid regex")
        });

        let source = cmd.source();
        let unquoted: String = extract_unquoted_chars(source)
            .into_iter()
            .map(|(_, ch)| ch)
            .collect();
        if VAR_REDIR_RE.is_match(&unquoted) {
            analysis.add_risk(SecurityRisk::new(
                RiskKind::DangerousVariables,
                "variable adjacent to a pipe or redirect may allow command injection if the \
                 variable contains newlines",
            ));
        }
    }
}

/// Detects newline injection attempts.
pub struct NewlineInjectionAnalyzer;

impl Analyzer for NewlineInjectionAnalyzer {
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis) {
        // Check for literal \n followed by what looks like a command
        #[allow(clippy::expect_used)]
        static NEWLINE_CMD_RE: Lazy<Regex> =
            Lazy::new(|| Regex::new(r#"\\n\s*[a-zA-Z]+"#).expect("valid regex"));

        // Check in double-quoted strings and ANSI-C quotes
        for token in cmd.tokens() {
            let text = match token.kind {
                TokenKind::DoubleQuoted | TokenKind::AnsiCQuoted => &token.text,
                _ => continue,
            };

            if NEWLINE_CMD_RE.is_match(text) {
                analysis.add_risk(
                    SecurityRisk::new(
                        RiskKind::NewlineInjection,
                        "newline escape followed by text may inject commands in some contexts",
                    )
                    .with_span(token.span)
                    .with_matched_text(text),
                );
            }
        }
    }
}

/// Detects IFS manipulation.
pub struct IfsInjectionAnalyzer;

impl Analyzer for IfsInjectionAnalyzer {
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis) {
        // Check for IFS assignment or $IFS usage in unquoted context.
        // `echo "IFS=:"` is safe (literal string), but `IFS=: cmd` is dangerous.
        let source = cmd.source();
        let unquoted: String = extract_unquoted_chars(source)
            .into_iter()
            .map(|(_, ch)| ch)
            .collect();

        if unquoted.contains("IFS=") || unquoted.contains("$IFS") {
            analysis.add_risk(SecurityRisk::new(
                RiskKind::IfsInjection,
                "IFS manipulation can alter word splitting behavior",
            ));
        }
    }
}

/// Detects access to /proc/*/environ.
pub struct ProcEnvironAnalyzer;

impl Analyzer for ProcEnvironAnalyzer {
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis) {
        #[allow(clippy::expect_used)]
        static PROC_ENVIRON_RE: Lazy<Regex> =
            Lazy::new(|| Regex::new(r#"/proc/[^/]+/environ"#).expect("valid regex"));

        let source = cmd.source();
        if PROC_ENVIRON_RE.is_match(source) {
            analysis.add_risk(SecurityRisk::new(
                RiskKind::ProcEnvironAccess,
                "accessing /proc/*/environ can expose sensitive environment variables",
            ));
        }
    }
}

/// Detects backslash-escaped whitespace outside quotes (line-continuation injection).
pub struct BackslashEscapedWhitespaceAnalyzer;

impl Analyzer for BackslashEscapedWhitespaceAnalyzer {
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis) {
        #[allow(clippy::expect_used)]
        static BS_WS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\\[ \t]").expect("valid regex"));

        // Check unquoted and double-quoted tokens (single-quoted is literal)
        for token in cmd.tokens() {
            if matches!(token.kind, TokenKind::SingleQuoted) {
                continue;
            }
            if matches!(
                token.kind,
                TokenKind::Word | TokenKind::DoubleQuoted | TokenKind::Operator
            ) && BS_WS_RE.is_match(&token.text)
            {
                analysis.add_risk(
                    SecurityRisk::new(
                        RiskKind::BackslashEscapedWhitespace,
                        "backslash before whitespace may inject line continuation",
                    )
                    .with_span(token.span)
                    .with_matched_text(&token.text),
                );
                return;
            }
        }
    }
}

/// Detects backslash-escaped shell operators outside quotes.
pub struct BackslashEscapedOperatorsAnalyzer;

impl Analyzer for BackslashEscapedOperatorsAnalyzer {
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis) {
        #[allow(clippy::expect_used)]
        static BS_OP_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\\[;|&<>]").expect("valid regex"));

        for token in cmd.tokens() {
            if matches!(
                token.kind,
                TokenKind::SingleQuoted | TokenKind::DoubleQuoted
            ) {
                continue;
            }
            if BS_OP_RE.is_match(&token.text) {
                analysis.add_risk(
                    SecurityRisk::new(
                        RiskKind::BackslashEscapedOperators,
                        "backslash-escaped operator may bypass naive quote stripping",
                    )
                    .with_span(token.span)
                    .with_matched_text(&token.text),
                );
                return;
            }
        }
    }
}

/// Detects non-ASCII Unicode whitespace characters.
pub struct UnicodeWhitespaceAnalyzer;

impl Analyzer for UnicodeWhitespaceAnalyzer {
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis) {
        let source = cmd.source();
        for ch in source.chars() {
            if ch.is_whitespace() && !ch.is_ascii() {
                analysis.add_risk(
                    SecurityRisk::new(
                        RiskKind::UnicodeWhitespace,
                        format!(
                            "non-ASCII whitespace character U+{:04X} may be used for obfuscation",
                            ch as u32
                        ),
                    )
                    .with_matched_text(ch.to_string()),
                );
                return;
            }
        }
    }
}

/// Detects `#` not preceded by whitespace or start-of-line (potential comment injection).
pub struct MidWordHashAnalyzer;

impl Analyzer for MidWordHashAnalyzer {
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis) {
        let source = cmd.source();
        let unquoted = extract_unquoted_chars(source);
        for &(idx, ch) in &unquoted {
            if ch == '#' && idx > 0 {
                let prev = source.as_bytes()[idx - 1];
                if !prev.is_ascii_whitespace() {
                    analysis.add_risk(SecurityRisk::new(
                        RiskKind::MidWordHash,
                        "hash character not preceded by whitespace may indicate comment injection",
                    ));
                    return;
                }
            }
        }
    }
}

/// Detects brace expansion ({a,b} or {1..3}) outside quotes.
pub struct BraceExpansionAnalyzer;

impl Analyzer for BraceExpansionAnalyzer {
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis) {
        #[allow(clippy::expect_used)]
        static BRACE_RE: Lazy<Regex> =
            Lazy::new(|| Regex::new(r"\{[^}]*?(?:,|\.\.)[^}]*\}").expect("valid regex"));

        let source = cmd.source();
        let unquoted: String = extract_unquoted_chars(source)
            .into_iter()
            .map(|(_, ch)| ch)
            .collect();

        if BRACE_RE.is_match(&unquoted) {
            analysis.add_risk(SecurityRisk::new(
                RiskKind::BraceExpansion,
                "brace expansion outside quotes may generate unexpected arguments",
            ));
        }
    }
}

/// Detects dangerous zsh-specific commands.
pub struct ZshDangerousCommandsAnalyzer;

impl Analyzer for ZshDangerousCommandsAnalyzer {
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis) {
        // Zsh-only builtins. `mapfile` is excluded here (it's a bash builtin
        // already covered by EvalLikeBuiltinAnalyzer).
        const ZSH_CMDS: &[&str] = &[
            "zmodload", "emulate", "sysopen", "zcompile", "autoload", "sysread", "syswrite",
            "sysseek", "zpty", "ztcp", "zsocket", "zf_rm", "zf_mv", "zf_ln", "zf_chmod",
            "zf_chown", "zf_mkdir", "zf_rmdir", "zf_chgrp",
        ];

        let commands = cmd.extract_commands();
        for args in &commands {
            if let Some(cmd_name) = args.first()
                && ZSH_CMDS.contains(&cmd_name.as_str())
            {
                analysis.add_risk(SecurityRisk::new(
                    RiskKind::ZshDangerousCommands,
                    format!("{cmd_name} is a dangerous zsh-specific command"),
                ));
            }
        }
    }
}

/// Detects odd number of unescaped quotes after `#` on a line (comment/quote desync).
pub struct CommentQuoteDesyncAnalyzer;

impl Analyzer for CommentQuoteDesyncAnalyzer {
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis) {
        let source = cmd.source();
        for line in source.lines() {
            // Find first unquoted `#` using quote-state tracking
            let bytes = line.as_bytes();
            let len = bytes.len();
            let mut i = 0;
            let mut in_single_quote = false;
            let mut in_double_quote = false;
            let mut hash_pos = None;

            while i < len {
                let ch = bytes[i];
                if !in_single_quote && !in_double_quote && ch == b'\\' {
                    i += 2; // skip escaped char
                    continue;
                }
                if ch == b'\'' && !in_double_quote {
                    in_single_quote = !in_single_quote;
                    i += 1;
                    continue;
                }
                if ch == b'"' && !in_single_quote {
                    in_double_quote = !in_double_quote;
                    i += 1;
                    continue;
                }
                if !in_single_quote && !in_double_quote && ch == b'#' {
                    hash_pos = Some(i);
                    break;
                }
                i += 1;
            }

            if let Some(pos) = hash_pos {
                let after_hash = &line[pos + 1..];
                let mut single_count = 0i32;
                let mut double_count = 0i32;
                let mut prev_was_backslash = false;
                for ch in after_hash.chars() {
                    if prev_was_backslash {
                        prev_was_backslash = false;
                        continue;
                    }
                    if ch == '\\' {
                        prev_was_backslash = true;
                        continue;
                    }
                    if ch == '\'' {
                        single_count += 1;
                    } else if ch == '"' {
                        double_count += 1;
                    }
                }
                if single_count % 2 != 0 || double_count % 2 != 0 {
                    analysis.add_risk(
                        SecurityRisk::new(
                            RiskKind::CommentQuoteDesync,
                            "odd number of quotes after # may desync parser quote tracking",
                        )
                        .with_matched_text(line),
                    );
                    return;
                }
            }
        }
    }
}

/// Detects literal newline followed by `#` inside double-quoted tokens.
pub struct QuotedNewlineHashAnalyzer;

impl Analyzer for QuotedNewlineHashAnalyzer {
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis) {
        for token in cmd.tokens() {
            if token.kind != TokenKind::DoubleQuoted {
                continue;
            }
            // Check for literal newline followed by #
            if token.text.contains('\n') {
                let lines: Vec<&str> = token.text.split('\n').collect();
                for line in lines.iter().skip(1) {
                    let trimmed = line.trim_start();
                    if trimmed.starts_with('#') {
                        analysis.add_risk(
                            SecurityRisk::new(
                                RiskKind::QuotedNewlineHash,
                                "newline followed by # inside double quotes may be interpreted as comment",
                            )
                            .with_span(token.span)
                            .with_matched_text(&token.text),
                        );
                        return;
                    }
                }
            }
        }
    }
}

// =============================================================================
// Ask Phase Analyzers
// =============================================================================

/// Detects dangerous substitutions ($(), ${}, <(), etc.).
pub struct DangerousSubstitutionAnalyzer;

impl Analyzer for DangerousSubstitutionAnalyzer {
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis) {
        for token in cmd.tokens() {
            match token.kind {
                TokenKind::CommandSubstitution => {
                    analysis.add_risk(
                        SecurityRisk::new(
                            RiskKind::DangerousSubstitution,
                            "command substitution executes embedded command",
                        )
                        .with_span(token.span)
                        .with_matched_text(&token.text),
                    );
                }
                TokenKind::ProcessSubstitution => {
                    analysis.add_risk(
                        SecurityRisk::new(
                            RiskKind::DangerousSubstitution,
                            "process substitution executes embedded command",
                        )
                        .with_span(token.span)
                        .with_matched_text(&token.text),
                    );
                }
                TokenKind::VariableExpansion => {
                    // Complex expansions like ${VAR:-default} can execute code
                    if token.text.contains(":-")
                        || token.text.contains(":+")
                        || token.text.contains(":?")
                        || token.text.contains("//")
                        || token.text.contains("%%")
                        || token.text.contains("##")
                    {
                        analysis.add_risk(
                            SecurityRisk::new(
                                RiskKind::DangerousSubstitution,
                                "complex parameter expansion may have side effects",
                            )
                            .with_span(token.span)
                            .with_matched_text(&token.text),
                        );
                    }
                }
                _ => {}
            }
        }
    }
}

/// Detects malformed tokens.
pub struct MalformedTokensAnalyzer;

impl Analyzer for MalformedTokensAnalyzer {
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis) {
        // Check for parse errors in the tree
        if cmd.has_errors() {
            analysis.add_risk(
                SecurityRisk::new(
                    RiskKind::MalformedTokens,
                    "command contains syntax errors which may indicate obfuscation",
                )
                .with_level(RiskLevel::Low),
            );
        }

        // Check for unbalanced brackets/quotes with quote-context awareness.
        // Skip counting inside single quotes (everything is literal there).
        let source = cmd.source();
        let mut paren_depth = 0i32;
        let mut brace_depth = 0i32;
        let mut bracket_depth = 0i32;
        let mut in_single_quote = false;
        let mut in_double_quote = false;
        let mut in_escape = false;

        for ch in source.chars() {
            if in_escape {
                in_escape = false;
                continue;
            }
            if ch == '\\' && !in_single_quote {
                in_escape = true;
                continue;
            }
            if ch == '\'' && !in_double_quote {
                in_single_quote = !in_single_quote;
                continue;
            }
            if ch == '"' && !in_single_quote {
                in_double_quote = !in_double_quote;
                continue;
            }
            // Skip bracket counting inside single quotes
            if in_single_quote {
                continue;
            }
            match ch {
                '(' => paren_depth += 1,
                ')' => paren_depth -= 1,
                '{' => brace_depth += 1,
                '}' => brace_depth -= 1,
                '[' => bracket_depth += 1,
                ']' => bracket_depth -= 1,
                _ => {}
            }
        }

        if paren_depth != 0 || brace_depth != 0 || bracket_depth != 0 {
            analysis.add_risk(
                SecurityRisk::new(RiskKind::MalformedTokens, "unbalanced brackets detected")
                    .with_level(RiskLevel::Low),
            );
        }
    }
}

/// Detects sensitive file redirections.
pub struct SensitiveRedirectAnalyzer;

impl Analyzer for SensitiveRedirectAnalyzer {
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis) {
        const SENSITIVE_PATHS: &[&str] = &[
            "/etc/passwd",
            "/etc/shadow",
            "/etc/sudoers",
            "~/.ssh/",
            ".ssh/",
            "id_rsa",
            "id_ed25519",
            ".env",
            ".netrc",
            ".npmrc",
            ".pypirc",
            "credentials",
            "secrets",
            "/dev/tcp",
            "/dev/udp",
            // Shell/editor config files
            ".bashrc",
            ".bash_profile",
            ".zshrc",
            ".zprofile",
            ".profile",
            // Git config
            ".gitconfig",
            ".gitmodules",
            // Tool config
            ".ripgreprc",
            ".mcp.json",
            ".coco.json",
            ".claude.json",
        ];

        if let Some(tree) = cmd.tree() {
            let redirects = extract_redirects_from_tree(tree, cmd.source());
            for redirect in redirects {
                // Skip fd-to-fd duplications like 2>&1 or 1>&2
                if redirect.kind == crate::redirects::RedirectKind::Duplicate
                    && redirect.target.parse::<i32>().is_ok()
                {
                    continue;
                }

                for sensitive in SENSITIVE_PATHS.iter() {
                    if redirect.target.contains(sensitive) {
                        let direction = if redirect.kind.is_output() {
                            "writing to"
                        } else {
                            "reading from"
                        };
                        analysis.add_risk(
                            SecurityRisk::new(
                                RiskKind::SensitiveRedirect,
                                format!("{direction} sensitive path: {}", redirect.target),
                            )
                            .with_span(redirect.span),
                        );
                    }
                }

                // Variable target: redirect to $VAR is suspicious
                if redirect.target.starts_with('$') {
                    analysis.add_risk(
                        SecurityRisk::new(
                            RiskKind::SensitiveRedirect,
                            "redirect target uses variable expansion",
                        )
                        .with_span(redirect.span),
                    );
                }

                // Check for /dev/tcp and /dev/udp (network redirects)
                if redirect.target.starts_with("/dev/tcp")
                    || redirect.target.starts_with("/dev/udp")
                {
                    analysis.add_risk(
                        SecurityRisk::new(
                            RiskKind::NetworkExfiltration,
                            format!("network redirection via {}", redirect.target),
                        )
                        .with_span(redirect.span),
                    );
                }
            }
        }
    }
}

/// Detects network exfiltration attempts.
pub struct NetworkExfiltrationAnalyzer;

impl Analyzer for NetworkExfiltrationAnalyzer {
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis) {
        const EXFIL_CMDS: &[&str] = &[
            "curl", "wget", "nc", "netcat", "ncat", "telnet", "ssh", "scp", "rsync", "ftp",
        ];

        let commands = cmd.extract_commands();
        for args in &commands {
            let cmd_name = args.first().map(String::as_str).unwrap_or("");

            if EXFIL_CMDS.contains(&cmd_name) {
                // Check for data being sent
                let has_data_flag = args.iter().any(|a| {
                    a == "-d"
                        || a == "--data"
                        || a == "-X"
                        || a == "POST"
                        || a == "-F"
                        || a == "--form"
                });

                // Check for piped input
                if let Some(tree) = cmd.tree() {
                    let segments = extract_segments_from_tree(tree, cmd.source());
                    let is_piped = segments
                        .iter()
                        .any(|s| s.command_name() == Some(cmd_name) && s.is_piped);

                    if has_data_flag || is_piped {
                        analysis.add_risk(SecurityRisk::new(
                            RiskKind::NetworkExfiltration,
                            format!("{cmd_name} command may exfiltrate data"),
                        ));
                    }
                }
            }
        }
    }
}

/// Detects privilege escalation attempts.
pub struct PrivilegeEscalationAnalyzer;

impl Analyzer for PrivilegeEscalationAnalyzer {
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis) {
        const PRIV_ESC_CMDS: &[&str] =
            &["sudo", "su", "doas", "pkexec", "gksudo", "kdesudo", "runas"];

        let commands = cmd.extract_commands();
        for args in &commands {
            let cmd_name = args.first().map(String::as_str).unwrap_or("");

            if PRIV_ESC_CMDS.contains(&cmd_name) {
                analysis.add_risk(SecurityRisk::new(
                    RiskKind::PrivilegeEscalation,
                    format!("{cmd_name} command requests elevated privileges"),
                ));
            }

            // Check for setuid/setgid operations
            if cmd_name == "chmod" {
                for arg in args.iter().skip(1) {
                    if arg.contains("s")
                        && (arg.starts_with("u+") || arg.starts_with("g+") || arg.starts_with('+'))
                    {
                        analysis.add_risk(SecurityRisk::new(
                            RiskKind::PrivilegeEscalation,
                            "chmod with setuid/setgid bit",
                        ));
                    }
                    // Numeric mode with setuid/setgid
                    if arg.len() == 4 && arg.chars().all(|c| c.is_ascii_digit()) {
                        let first_digit: i32 = arg[..1].parse().unwrap_or(0);
                        if first_digit >= 4 {
                            analysis.add_risk(SecurityRisk::new(
                                RiskKind::PrivilegeEscalation,
                                "chmod with setuid/setgid bit (numeric mode)",
                            ));
                        }
                    }
                }
            }
        }
    }
}

/// Detects file system tampering.
pub struct FileSystemTamperingAnalyzer;

impl Analyzer for FileSystemTamperingAnalyzer {
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis) {
        let commands = cmd.extract_commands();

        for args in &commands {
            let cmd_name = args.first().map(String::as_str).unwrap_or("");

            match cmd_name {
                "rm" => {
                    // Check for dangerous rm flags. Only match combined short
                    // flags (e.g. `-rf`, `-fr`) that start with `-` and are
                    // short-flag clusters — never match long options like
                    // `--preserve` which happen to contain `r`.
                    let is_short_flag_cluster =
                        |a: &str| a.starts_with('-') && !a.starts_with("--") && a.len() > 1;
                    let has_recursive = args.iter().any(|a| {
                        a == "-r"
                            || a == "-R"
                            || a == "--recursive"
                            || (is_short_flag_cluster(a) && a[1..].contains(['r', 'R']))
                    });
                    let has_force = args.iter().any(|a| {
                        a == "-f"
                            || a == "--force"
                            || (is_short_flag_cluster(a) && a[1..].contains('f'))
                    });

                    if has_recursive && has_force {
                        analysis.add_risk(SecurityRisk::new(
                            RiskKind::FileSystemTampering,
                            "rm -rf can recursively delete files without confirmation",
                        ));
                    }

                    // Check for dangerous paths
                    for arg in args.iter().skip(1) {
                        if !arg.starts_with('-')
                            && (arg == "/" || arg == "/*" || arg == "~" || arg == "~/*")
                        {
                            analysis.add_risk(
                                SecurityRisk::new(
                                    RiskKind::FileSystemTampering,
                                    format!("rm targeting dangerous path: {arg}"),
                                )
                                .with_level(RiskLevel::Critical),
                            );
                        }
                    }
                }

                "mkfs" | "dd" | "shred" | "wipefs" => {
                    analysis.add_risk(SecurityRisk::new(
                        RiskKind::FileSystemTampering,
                        format!("{cmd_name} can cause data loss"),
                    ));
                }

                "chown" | "chgrp" => {
                    // Check for recursive operations
                    if args.iter().any(|a| a == "-R" || a == "--recursive") {
                        analysis.add_risk(SecurityRisk::new(
                            RiskKind::FileSystemTampering,
                            format!("recursive {cmd_name} can change ownership of many files"),
                        ));
                    }
                }

                _ => {}
            }
        }
    }
}

/// Detects code execution risks.
pub struct CodeExecutionAnalyzer;

impl Analyzer for CodeExecutionAnalyzer {
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis) {
        let commands = cmd.extract_commands();

        for args in &commands {
            let cmd_name = args.first().map(String::as_str).unwrap_or("");

            // Direct code execution commands
            if matches!(cmd_name, "eval" | "exec") {
                analysis.add_risk(SecurityRisk::new(
                    RiskKind::CodeExecution,
                    format!("{cmd_name} executes arbitrary code"),
                ));
            }

            // Shell invocations with -c flag
            if matches!(cmd_name, "bash" | "sh" | "zsh" | "fish")
                && args.iter().any(|a| a == "-c" || a == "-lc")
            {
                analysis.add_risk(SecurityRisk::new(
                    RiskKind::CodeExecution,
                    format!("{cmd_name} -c executes shell code"),
                ));
            }

            // Interpreter invocations with -c flag or code arguments
            if matches!(
                cmd_name,
                "python" | "python2" | "python3" | "perl" | "ruby" | "php" | "lua"
            ) && args.iter().any(|a| a == "-c" || a == "-e")
            {
                analysis.add_risk(SecurityRisk::new(
                    RiskKind::CodeExecution,
                    format!("{cmd_name} executes inline code"),
                ));
            }

            // Node/Deno/tsx with -e/--eval flag
            if matches!(cmd_name, "node" | "deno" | "tsx")
                && args.iter().any(|a| a == "-e" || a == "--eval")
            {
                analysis.add_risk(SecurityRisk::new(
                    RiskKind::CodeExecution,
                    format!("{cmd_name} --eval executes code"),
                ));
            }

            // Package runners that execute arbitrary code
            if matches!(cmd_name, "npx" | "bunx") {
                analysis.add_risk(SecurityRisk::new(
                    RiskKind::CodeExecution,
                    format!("{cmd_name} downloads and executes packages"),
                ));
            }

            // Package manager run scripts
            if matches!(cmd_name, "npm" | "yarn" | "pnpm" | "bun")
                && args.get(1).is_some_and(|a| a == "run")
            {
                analysis.add_risk(SecurityRisk::new(
                    RiskKind::CodeExecution,
                    format!("{cmd_name} run executes project scripts"),
                ));
            }

            // ssh with remote command
            if cmd_name == "ssh" {
                // ssh host command — if there are non-flag args beyond the host,
                // a remote command is being executed
                let non_flag_count = args.iter().skip(1).filter(|a| !a.starts_with('-')).count();
                if non_flag_count >= 2 {
                    analysis.add_risk(SecurityRisk::new(
                        RiskKind::CodeExecution,
                        "ssh executes remote command",
                    ));
                }
            }
        }
    }
}

/// Detects unsafe heredoc usage in command substitutions.
///
/// A heredoc with an unquoted delimiter (`<<EOF`) expands variables, which is
/// dangerous inside command substitutions (`$(... <<EOF ...)`). Quoted delimiters
/// (`<<'EOF'` or `<<\EOF`) suppress expansion and are safe.
pub struct HeredocSubstitutionAnalyzer;

impl HeredocSubstitutionAnalyzer {
    /// Check if a `<<` heredoc operator at `pos` has an unquoted delimiter.
    ///
    /// Skips the optional `-` after `<<` then checks if the delimiter starts
    /// with a quote character.
    fn is_unquoted_heredoc(source: &str, heredoc_pos: usize) -> bool {
        let after = &source[heredoc_pos..];

        // Skip "<<"
        let rest = after.strip_prefix("<<").unwrap_or(after);
        // Skip optional "-" (for <<-)
        let rest = rest.strip_prefix('-').unwrap_or(rest);
        // Skip whitespace
        let rest = rest.trim_start_matches([' ', '\t']);

        // A quoted delimiter starts with ', ", or \
        !(rest.starts_with('\'') || rest.starts_with('"') || rest.starts_with('\\'))
    }
}

impl Analyzer for HeredocSubstitutionAnalyzer {
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis) {
        let source = cmd.source();
        let bytes = source.as_bytes();

        // Track $( nesting depth to know if we're inside a command substitution
        let mut i = 0;
        let mut cmd_subst_depth = 0i32;

        while i < bytes.len() {
            // Detect $( — enter command substitution
            if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
                cmd_subst_depth += 1;
                i += 2;
                continue;
            }
            // Detect ) — leave command substitution
            if bytes[i] == b')' && cmd_subst_depth > 0 {
                cmd_subst_depth -= 1;
                i += 1;
                continue;
            }
            // Detect << (but not <<<)
            if bytes[i] == b'<'
                && i + 1 < bytes.len()
                && bytes[i + 1] == b'<'
                && (i + 2 >= bytes.len() || bytes[i + 2] != b'<')
            {
                if cmd_subst_depth > 0 && Self::is_unquoted_heredoc(source, i) {
                    analysis.add_risk(
                        SecurityRisk::new(
                            RiskKind::UnsafeHeredocSubstitution,
                            "unquoted heredoc delimiter inside command substitution allows variable expansion",
                        ),
                    );
                }
                // Skip past the << so we don't double-count
                i += 2;
                continue;
            }
            // Skip single-quoted strings entirely (no expansion inside)
            if bytes[i] == b'\'' {
                i += 1;
                while i < bytes.len() && bytes[i] != b'\'' {
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1;
                }
                continue;
            }

            i += 1;
        }
    }
}

/// Detects excess unbalanced closing braces/brackets after stripping quoted content.
///
/// Unmatched `)`, `}`, `]` after quote removal indicate possible
/// brace-based obfuscation or injection (CC: checkExcessClosingBraces).
pub struct ExcessClosingBracesAnalyzer;

impl Analyzer for ExcessClosingBracesAnalyzer {
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis) {
        let source = cmd.source();
        // Strip single-quoted and double-quoted content, then check balance
        let mut stripped = String::with_capacity(source.len());
        let mut in_single = false;
        let mut in_double = false;
        let mut chars = source.chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                '\\' if !in_single => {
                    chars.next();
                }
                '\'' if !in_double => {
                    in_single = !in_single;
                }
                '"' if !in_single => {
                    in_double = !in_double;
                }
                _ if !in_single && !in_double => stripped.push(ch),
                _ => {}
            }
        }

        // Count bracket balance in stripped content
        let mut paren: i32 = 0;
        let mut brace: i32 = 0;
        let mut bracket: i32 = 0;
        for ch in stripped.chars() {
            match ch {
                '(' => paren += 1,
                ')' => paren -= 1,
                '{' => brace += 1,
                '}' => brace -= 1,
                '[' => bracket += 1,
                ']' => bracket -= 1,
                _ => {}
            }
            // If any counter goes negative, we have excess closing braces
            if paren < 0 || brace < 0 || bracket < 0 {
                analysis.add_risk(SecurityRisk::new(
                    RiskKind::ExcessClosingBraces,
                    "Excess unbalanced closing braces/brackets after quote stripping".to_string(),
                ));
                return;
            }
        }
    }
}

// =============================================================================
// New Deny Phase Analyzers
// =============================================================================

/// Eval-like builtins that execute arguments as shell code.
///
/// These builtins can execute arbitrary code through their arguments,
/// making them dangerous when the arguments are not fully controlled.
///
/// Safe-path exceptions:
/// - `command -v/-V foo` (existence check only)
/// - `fc -l/-ln` (list history only)
/// - `compgen -c/-f/-v` (list completions only)
pub struct EvalLikeBuiltinAnalyzer;

impl EvalLikeBuiltinAnalyzer {
    const EVAL_LIKE_BUILTINS: &[&str] = &[
        "eval",
        "source",
        ".",
        "exec",
        "command",
        "builtin",
        "fc",
        "coproc",
        "noglob",
        "nocorrect",
        "trap",
        "enable",
        "mapfile",
        "readarray",
        "hash",
        "bind",
        "complete",
        "compgen",
        "alias",
        "let",
    ];

    /// Check if the command is a safe exception that should not trigger.
    fn is_safe_exception(cmd_name: &str, args: &[String]) -> bool {
        match cmd_name {
            // `command -v foo` / `command -V foo` — existence check
            "command" => args
                .get(1)
                .is_some_and(|a| matches!(a.as_str(), "-v" | "-V")),
            // `fc -l` / `fc -ln` — list history
            "fc" => args
                .get(1)
                .is_some_and(|a| a == "-l" || a == "-ln" || a == "-nl"),
            // `compgen` — safe only if flags don't include callback/file/wordlist
            // execution flags: -C (callback), -F (function), -W (wordlist).
            // Any flag cluster containing these uppercase letters is dangerous.
            "compgen" => args
                .iter()
                .skip(1)
                .filter(|a| a.starts_with('-'))
                .all(|a| !a[1..].contains(['C', 'F', 'W'])),
            _ => false,
        }
    }
}

impl Analyzer for EvalLikeBuiltinAnalyzer {
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis) {
        let commands = cmd.extract_commands();
        for args in &commands {
            let cmd_name = args.first().map(String::as_str).unwrap_or("");
            if Self::EVAL_LIKE_BUILTINS.contains(&cmd_name)
                && !Self::is_safe_exception(cmd_name, args)
            {
                analysis.add_risk(SecurityRisk::new(
                    RiskKind::EvalLikeBuiltin,
                    format!("{cmd_name} is an eval-like builtin that can execute code through its arguments"),
                ));
            }
        }
    }
}

/// Detects builtins whose NAME arguments trigger array subscript evaluation.
///
/// When builtins like `test -v`,
/// `printf -v`, `read -a`, `unset -v`, `wait -p` receive a NAME argument,
/// bash re-parses it and evaluates array subscripts. This means
/// `read 'a[$(id)]'` silently executes `id`.
pub struct SubscriptEvalAnalyzer;

impl SubscriptEvalAnalyzer {
    /// (builtin, dangerous_flags) pairs.
    const FLAGGED_BUILTINS: &[(&str, &[&str])] = &[
        ("test", &["-v", "-R"]),
        ("[", &["-v", "-R"]),
        ("[[", &["-v", "-R"]),
        ("printf", &["-v"]),
        ("read", &["-a"]),
        ("unset", &["-v"]),
        ("wait", &["-p"]),
    ];

    /// Builtins where ALL non-flag positional args are NAMEs.
    const BARE_NAME_BUILTINS: &[&str] = &["read", "unset"];

    /// `read` flags that take a data argument (not a NAME).
    const READ_DATA_FLAGS: &[&str] = &["-p", "-d", "-n", "-N", "-t", "-u", "-i"];

    fn arg_contains_subscript(arg: &str) -> bool {
        arg.contains('[')
    }
}

impl Analyzer for SubscriptEvalAnalyzer {
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis) {
        // Use both extracted commands AND source-level scanning because
        // tree-sitter may not preserve `arr[0]` as a single token.
        let commands = cmd.extract_commands();
        let source = cmd.source();

        for args in &commands {
            let cmd_name = args.first().map(String::as_str).unwrap_or("");

            // Check flagged builtins: flag + next arg with [
            for &(builtin, danger_flags) in Self::FLAGGED_BUILTINS {
                if cmd_name != builtin {
                    continue;
                }
                for (i, arg) in args.iter().enumerate().skip(1) {
                    // Separate form: -v NAME (next arg)
                    if danger_flags.contains(&arg.as_str())
                        && args
                            .get(i + 1)
                            .is_some_and(|next| Self::arg_contains_subscript(next))
                    {
                        analysis.add_risk(SecurityRisk::new(
                            RiskKind::SubscriptEval,
                            format!(
                                "{cmd_name} {arg} with array subscript triggers code evaluation"
                            ),
                        ));
                        return;
                    }
                    // Fused form: -vNAME or combined flags -ra where r triggers
                    if arg.starts_with('-')
                        && arg.len() > 2
                        && danger_flags
                            .iter()
                            .any(|f| arg.contains(f.trim_start_matches('-')))
                        && Self::arg_contains_subscript(arg)
                    {
                        analysis.add_risk(SecurityRisk::new(
                            RiskKind::SubscriptEval,
                            format!("{cmd_name} fused flag {arg} with array subscript triggers code evaluation"),
                        ));
                        return;
                    }
                }
            }

            // Check bare NAME builtins: positional args with [
            if Self::BARE_NAME_BUILTINS.contains(&cmd_name) {
                let mut skip_next = false;
                for arg in args.iter().skip(1) {
                    if skip_next {
                        skip_next = false;
                        continue;
                    }
                    if cmd_name == "read" && Self::READ_DATA_FLAGS.contains(&arg.as_str()) {
                        skip_next = true;
                        continue;
                    }
                    if arg.starts_with('-') {
                        continue;
                    }
                    if Self::arg_contains_subscript(arg) {
                        analysis.add_risk(SecurityRisk::new(
                            RiskKind::SubscriptEval,
                            format!("{cmd_name} with array subscript in NAME argument triggers code evaluation"),
                        ));
                        return;
                    }
                }
            }
        }

        // Fallback: source-level scan for cases where tree-sitter splits
        // `arr[0]` across multiple AST nodes.
        let tokens: Vec<&str> = source.split_whitespace().collect();
        for &(builtin, danger_flags) in Self::FLAGGED_BUILTINS {
            for (i, &token) in tokens.iter().enumerate() {
                if token != builtin {
                    continue;
                }
                // Look for danger flag followed by token with [
                for &flag in danger_flags {
                    for j in (i + 1)..tokens.len() {
                        if tokens[j] == flag
                            && tokens
                                .get(j + 1)
                                .is_some_and(|t| Self::arg_contains_subscript(t))
                        {
                            analysis.add_risk(SecurityRisk::new(
                                RiskKind::SubscriptEval,
                                format!(
                                    "{builtin} {flag} with array subscript triggers code evaluation"
                                ),
                            ));
                            return;
                        }
                    }
                }
            }
        }

        // Fallback for bare NAME builtins
        for &builtin in Self::BARE_NAME_BUILTINS {
            for (i, &token) in tokens.iter().enumerate() {
                if token != builtin {
                    continue;
                }
                for &tok in &tokens[i + 1..] {
                    if tok.starts_with('-') {
                        continue;
                    }
                    if Self::arg_contains_subscript(tok) {
                        analysis.add_risk(SecurityRisk::new(
                            RiskKind::SubscriptEval,
                            format!(
                                "{builtin} with array subscript in NAME triggers code evaluation"
                            ),
                        ));
                        return;
                    }
                }
            }
        }
    }
}

/// Detects arithmetic comparisons in `[[` that evaluate subscripts.
///
/// In `[[ ARG -eq ARG ]]`, both operands
/// are arithmetically evaluated. If an operand contains `[`, it may execute
/// code via array subscript evaluation: `[[ arr[$(cmd)] -eq 0 ]]`.
pub struct ArithComparisonAnalyzer;

impl ArithComparisonAnalyzer {
    const ARITH_OPS: &[&str] = &["-eq", "-ne", "-lt", "-le", "-gt", "-ge"];
}

impl Analyzer for ArithComparisonAnalyzer {
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis) {
        // Tree-sitter treats [[ ]] as a compound command, not a simple command,
        // so extract_commands() won't return it. Instead, scan the source tokens
        // for the pattern: [[ ... ARG -OP ARG ... ]]
        let source = cmd.source();

        // Quick check: source must contain [[ and an arith op
        if !source.contains("[[") {
            return;
        }

        // Tokenize and scan for [[ ... -op ... ]] patterns
        let tokens: Vec<&str> = source.split_whitespace().collect();
        let mut in_bracket = false;

        for (i, &token) in tokens.iter().enumerate() {
            if token == "[[" {
                in_bracket = true;
                continue;
            }
            if token == "]]" {
                in_bracket = false;
                continue;
            }
            if !in_bracket {
                continue;
            }

            if Self::ARITH_OPS.contains(&token) {
                let left = if i > 0 { tokens.get(i - 1) } else { None };
                let right = tokens.get(i + 1);
                let left_has_subscript = left.is_some_and(|a| a.contains('['));
                let right_has_subscript = right.is_some_and(|a| a.contains('['));
                if left_has_subscript || right_has_subscript {
                    analysis.add_risk(SecurityRisk::new(
                        RiskKind::ArithComparison,
                        format!(
                            "[[ arithmetic {token} with array subscript triggers code evaluation"
                        ),
                    ));
                    return;
                }
            }
        }
    }
}

/// Detects `VAR=value` prefix before a dangerous inner command.
///
/// Env var prefixes like `FOO=bar eval "malicious"` allow the inner command
/// to bypass detection if only the first token is checked. This analyzer
/// strips env var assignments and checks the inner command.
pub struct EnvVarPrefixAnalyzer;

impl EnvVarPrefixAnalyzer {
    /// Commands that are dangerous when hidden behind env var assignments.
    const DANGEROUS_INNER_CMDS: &[&str] = &[
        "eval", "source", ".", "exec", "command", "builtin", "trap", "enable", "bash", "sh", "zsh",
        "python", "python2", "python3", "perl", "ruby", "php", "node", "deno", "lua", "npx",
        "bunx",
    ];
}

impl Analyzer for EnvVarPrefixAnalyzer {
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis) {
        // Tree-sitter separates env var assignments from the command in the AST,
        // so extract_commands() returns only the command without prefixes.
        // We need to scan the source text to detect env prefixes.
        let source = cmd.source();

        // Process each semicolon/pipe-separated segment
        for segment in source.split([';', '\n']) {
            let segment = segment.trim();
            if segment.is_empty() {
                continue;
            }

            let tokens: Vec<&str> = segment.split_whitespace().collect();
            let mut inner_start = 0;

            // Skip leading VAR=val tokens
            for &token in &tokens {
                if token.contains('=') {
                    let key = token.split('=').next().unwrap_or("");
                    if !key.is_empty()
                        && key.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                        && key.chars().all(|c| {
                            c.is_ascii_alphanumeric()
                                || c == '_'
                                || c == '['
                                || c == ']'
                                || c == '+'
                        })
                    {
                        inner_start += 1;
                        continue;
                    }
                }
                break;
            }

            if inner_start == 0 {
                continue;
            }

            // Check the inner command after env vars
            if let Some(&inner_cmd) = tokens.get(inner_start)
                && Self::DANGEROUS_INNER_CMDS.contains(&inner_cmd)
            {
                analysis.add_risk(SecurityRisk::new(
                    RiskKind::EvalLikeBuiltin,
                    format!("environment variable prefix hides dangerous command: {inner_cmd}"),
                ));
            }
        }
    }
}

/// Detects dangerous operations targeting sensitive system paths.
///
/// Catches file-modifying commands (`rm`, `mv`, `cp`, `chmod`, `chown`, `touch`,
/// `mkdir`) that target root-level directories, system paths, or config directories.
pub struct DangerousPathAnalyzer;

impl DangerousPathAnalyzer {
    /// File-modifying commands to check.
    const MODIFYING_COMMANDS: &[&str] = &[
        "rm", "mv", "cp", "chmod", "chown", "touch", "mkdir", "rmdir", "ln",
    ];

    /// Dangerous root-level paths.
    const DANGEROUS_PATHS: &[&str] = &[
        "/", "/*", "~", "~/*", "/usr", "/tmp", "/etc", "/var", "/bin", "/sbin", "/lib", "/opt",
        "/srv", "/sys", "/proc", "/boot", "/dev", "/root", "/home",
    ];

    /// Dangerous config directories.
    const DANGEROUS_DIRS: &[&str] = &[
        ".git/", ".git", ".vscode/", ".vscode", ".idea/", ".idea", ".coco/", ".coco", ".claude/",
        ".claude",
    ];
}

impl Analyzer for DangerousPathAnalyzer {
    fn analyze(&self, cmd: &ParsedShell, analysis: &mut SecurityAnalysis) {
        let commands = cmd.extract_commands();
        for args in &commands {
            let cmd_name = args.first().map(String::as_str).unwrap_or("");
            if !Self::MODIFYING_COMMANDS.contains(&cmd_name) {
                continue;
            }

            for arg in args.iter().skip(1) {
                if arg.starts_with('-') {
                    continue; // skip flags
                }

                // Check dangerous root-level paths
                if Self::DANGEROUS_PATHS.contains(&arg.as_str()) {
                    analysis.add_risk(
                        SecurityRisk::new(
                            RiskKind::DangerousPath,
                            format!("{cmd_name} targeting dangerous path: {arg}"),
                        )
                        .with_level(RiskLevel::Critical),
                    );
                    return;
                }

                // Check if path starts with a dangerous root child
                for &dp in &Self::DANGEROUS_PATHS[4..] {
                    // skip /, /*, ~, ~/*
                    if arg.starts_with(dp)
                        && (arg.len() == dp.len() || arg.as_bytes().get(dp.len()) == Some(&b'/'))
                    {
                        analysis.add_risk(SecurityRisk::new(
                            RiskKind::DangerousPath,
                            format!("{cmd_name} targeting system path: {arg}"),
                        ));
                        return;
                    }
                }

                // Check dangerous config directories
                for &dd in Self::DANGEROUS_DIRS {
                    if arg == dd
                        || arg.starts_with(&format!("{dd}/"))
                        || arg.ends_with(&format!("/{dd}"))
                    {
                        analysis.add_risk(SecurityRisk::new(
                            RiskKind::DangerousPath,
                            format!("{cmd_name} targeting config directory: {arg}"),
                        ));
                        return;
                    }
                }
            }
        }
    }
}

/// Get all default analyzers.
pub fn default_analyzers() -> Vec<Box<dyn Analyzer>> {
    vec![
        // Deny phase
        Box::new(JqDangerAnalyzer),
        Box::new(ObfuscatedFlagsAnalyzer),
        Box::new(ShellMetacharactersAnalyzer),
        Box::new(DangerousVariablesAnalyzer),
        Box::new(NewlineInjectionAnalyzer),
        Box::new(IfsInjectionAnalyzer),
        Box::new(ProcEnvironAnalyzer),
        Box::new(BackslashEscapedWhitespaceAnalyzer),
        Box::new(BackslashEscapedOperatorsAnalyzer),
        Box::new(UnicodeWhitespaceAnalyzer),
        Box::new(MidWordHashAnalyzer),
        Box::new(BraceExpansionAnalyzer),
        Box::new(ZshDangerousCommandsAnalyzer),
        Box::new(CommentQuoteDesyncAnalyzer),
        Box::new(QuotedNewlineHashAnalyzer),
        Box::new(ExcessClosingBracesAnalyzer),
        Box::new(EvalLikeBuiltinAnalyzer),
        Box::new(SubscriptEvalAnalyzer),
        Box::new(ArithComparisonAnalyzer),
        Box::new(EnvVarPrefixAnalyzer),
        // Ask phase
        Box::new(HeredocSubstitutionAnalyzer),
        Box::new(DangerousSubstitutionAnalyzer),
        Box::new(MalformedTokensAnalyzer),
        Box::new(SensitiveRedirectAnalyzer),
        Box::new(NetworkExfiltrationAnalyzer),
        Box::new(PrivilegeEscalationAnalyzer),
        Box::new(FileSystemTamperingAnalyzer),
        Box::new(CodeExecutionAnalyzer),
        Box::new(DangerousPathAnalyzer),
    ]
}

#[cfg(test)]
#[path = "analyzers.test.rs"]
mod tests;
