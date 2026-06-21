//! Security analysis module for shell commands.
//!
//! This module provides comprehensive security analysis for shell commands,
//! detecting various risk patterns across two phases:
//!
//! - **Deny phase**: High-confidence injection patterns auto-denied by consumers
//! - **Ask phase**: Risks that require user approval
//!
//! # Example
//!
//! ```
//! use coco_shell_parser::{ShellParser, security};
//!
//! let mut parser = ShellParser::new();
//! let cmd = parser.parse("rm -rf /tmp/*");
//! let analysis = security::analyze(&cmd);
//!
//! if analysis.has_risks() {
//!     for risk in &analysis.risks {
//!         println!("{}", risk);
//!     }
//! }
//! ```

mod analyzers;
mod risks;
pub mod wrappers;

pub use analyzers::Analyzer;
pub use analyzers::ArithComparisonAnalyzer;
pub use analyzers::BackslashEscapedOperatorsAnalyzer;
pub use analyzers::BackslashEscapedWhitespaceAnalyzer;
pub use analyzers::BraceExpansionAnalyzer;
pub use analyzers::CodeExecutionAnalyzer;
pub use analyzers::CommentQuoteDesyncAnalyzer;
pub use analyzers::DangerousPathAnalyzer;
pub use analyzers::DangerousSubstitutionAnalyzer;
pub use analyzers::DangerousVariablesAnalyzer;
pub use analyzers::EnvVarPrefixAnalyzer;
pub use analyzers::EvalLikeBuiltinAnalyzer;
pub use analyzers::FileSystemTamperingAnalyzer;
pub use analyzers::HeredocSubstitutionAnalyzer;
pub use analyzers::IfsInjectionAnalyzer;
pub use analyzers::JqDangerAnalyzer;
pub use analyzers::MalformedTokensAnalyzer;
pub use analyzers::MidWordHashAnalyzer;
pub use analyzers::NetworkExfiltrationAnalyzer;
pub use analyzers::NewlineInjectionAnalyzer;
pub use analyzers::ObfuscatedFlagsAnalyzer;
pub use analyzers::PrivilegeEscalationAnalyzer;
pub use analyzers::ProcEnvironAnalyzer;
pub use analyzers::QuotedNewlineHashAnalyzer;
pub use analyzers::SensitiveRedirectAnalyzer;
pub use analyzers::ShellMetacharactersAnalyzer;
pub use analyzers::SingleQuoteBypassAnalyzer;
pub use analyzers::SubscriptEvalAnalyzer;
pub use analyzers::UnicodeWhitespaceAnalyzer;
pub use analyzers::ZshDangerousCommandsAnalyzer;
pub use analyzers::default_analyzers;
pub use risks::RiskKind;
pub use risks::RiskLevel;
pub use risks::RiskPhase;
pub use risks::SecurityAnalysis;
pub use risks::SecurityRisk;

use crate::parser::ParsedShell;
use crate::tokenizer::TokenKind;

/// Check if a command matches known-safe patterns that don't need full analysis.
///
/// Matches Claude Code's Phase A allow-list:
/// - Empty/whitespace commands
/// - Safe git commit messages (no variable expansion)
fn is_safe_pattern(cmd: &ParsedShell) -> bool {
    // Empty or whitespace-only command
    if cmd.source().trim().is_empty() {
        return true;
    }

    // Safe git commit: `git commit -m "..."` with no expansion in message
    let commands = cmd.extract_commands();
    if commands.len() == 1 {
        let args = &commands[0];
        if args.first().is_some_and(|c| c == "git")
            && args.get(1).is_some_and(|c| c == "commit")
            && args.iter().any(|a| a == "-m")
        {
            // Verify no tokens contain variable expansion or command substitution
            let has_expansion = cmd.tokens().iter().any(|t| {
                matches!(
                    t.kind,
                    TokenKind::CommandSubstitution
                        | TokenKind::ProcessSubstitution
                        | TokenKind::VariableExpansion
                )
            });
            if !has_expansion {
                return true;
            }
        }
    }

    false
}

/// Analyze a parsed command for security risks using all default analyzers.
///
/// The analysis pipeline is:
/// 1. **Layer 0**: Pre-check for single-quote bypass (early return on deny)
/// 2. **Safe pattern short-circuit**: Skip full analysis for known-safe patterns
/// 3. **Wrapper stripping**: Strip safe wrappers and analyze inner command
/// 4. **Full analyzer pipeline**: Run all default analyzers
pub fn analyze(cmd: &ParsedShell) -> SecurityAnalysis {
    let mut analysis = SecurityAnalysis::new();

    // Layer 0: Pre-check for quote bypass (highest priority)
    SingleQuoteBypassAnalyzer.analyze(cmd, &mut analysis);
    if analysis.is_auto_denied() {
        return analysis;
    }

    // Safe pattern short-circuit
    if is_safe_pattern(cmd) {
        return analysis;
    }

    // Full analyzer pipeline on the original command
    let analyzers = default_analyzers();
    for analyzer in &analyzers {
        analyzer.analyze(cmd, &mut analysis);
    }

    // Wrapper stripping: if the command has wrapper prefixes, also analyze
    // the inner command to catch risks hidden behind wrappers.
    let commands = cmd.extract_commands();
    if let Some(first_cmd) = commands.first()
        && let Some(inner_args) = wrappers::strip_all_wrappers(first_cmd)
    {
        let inner_source = inner_args.join(" ");
        let mut parser = crate::parser::ShellParser::new();
        let inner_cmd = parser.parse(&inner_source);
        for analyzer in &analyzers {
            analyzer.analyze(&inner_cmd, &mut analysis);
        }
    }

    analysis
}

/// Analyze a parsed command with a custom set of analyzers.
pub fn analyze_with(cmd: &ParsedShell, analyzers: &[Box<dyn Analyzer>]) -> SecurityAnalysis {
    let mut analysis = SecurityAnalysis::new();
    for analyzer in analyzers {
        analyzer.analyze(cmd, &mut analysis);
    }
    analysis
}

/// Quick check if a command has any security risks.
pub fn has_risks(cmd: &ParsedShell) -> bool {
    analyze(cmd).has_risks()
}

/// Quick check if a command requires user approval.
pub fn requires_approval(cmd: &ParsedShell) -> bool {
    analyze(cmd).requires_approval()
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
