//! Shell command parsing and security analysis.
//!
//! This crate provides comprehensive shell command parsing using tree-sitter
//! with a tokenizer fallback, plus security analysis to detect potentially
//! dangerous patterns.
//!
//! # Features
//!
//! - **Multi-layer parsing**: Tree-sitter AST parsing with tokenizer fallback
//! - **Safe command extraction**: Whitelist-based extraction of "word-only" commands
//! - **Pipe segment extraction**: Parse pipelines into individual segments
//! - **Redirection parsing**: Detect and classify shell redirections
//! - **Security analysis**: 24 risk types across 2 phases (Deny/Ask)
//!
//! # Quick Start
//!
//! ```
//! use coco_shell_parser::{ShellParser, security};
//!
//! // Parse a shell command
//! let mut parser = ShellParser::new();
//! let cmd = parser.parse("cat file.txt | grep pattern > output.txt");
//!
//! // Extract commands (safe extraction)
//! if let Some(commands) = cmd.try_extract_safe_commands() {
//!     for args in commands {
//!         println!("Command: {:?}", args);
//!     }
//! }
//!
//! // Analyze for security risks
//! let analysis = security::analyze(&cmd);
//! if analysis.has_risks() {
//!     for risk in &analysis.risks {
//!         println!("Risk: {}", risk);
//!     }
//! }
//! ```
//!
//! # Parsing Shell Invocations
//!
//! ```
//! use coco_shell_parser::ShellParser;
//!
//! let mut parser = ShellParser::new();
//!
//! // Parse shell invocation from argv
//! let argv = vec!["bash".into(), "-c".into(), "ls -la && pwd".into()];
//! if let Some(cmd) = parser.parse_shell_invocation(&argv) {
//!     let commands = cmd.extract_commands();
//!     // commands = [["ls", "-la"], ["pwd"]]
//! }
//! ```
//!
//! # Tokenization Only
//!
//! For lighter-weight parsing without tree-sitter:
//!
//! ```
//! use coco_shell_parser::Tokenizer;
//!
//! let tokenizer = Tokenizer::new();
//! let tokens = tokenizer.tokenize("echo 'hello world' $HOME").unwrap();
//!
//! for token in &tokens {
//!     println!("{:?}: {}", token.kind, token.text);
//! }
//! ```

mod error;
mod parser;
mod redirects;
mod segments;
mod tokenizer;

pub mod safety;
pub mod security;
pub mod summary;

// Re-export main types
pub use error::ParseError;
pub use error::Result;
pub use parser::ParsedShell;

// Re-export new summary and safety types
pub use parser::ShellParser;
pub use parser::ShellType;
pub use parser::detect_shell_type;
pub use parser::extract_shell_script;
pub use redirects::Redirect;
pub use redirects::RedirectKind;
pub use redirects::extract_redirects_from_tokens;
pub use redirects::extract_redirects_from_tree;
pub use safety::command_might_be_dangerous;
pub use safety::is_known_safe_command;
pub use segments::PipeSegment;
pub use segments::extract_segments_from_tokens;
pub use segments::extract_segments_from_tree;
pub use summary::CommandSummary;
pub use summary::parse_command;
pub use tokenizer::Span;
pub use tokenizer::Token;
pub use tokenizer::TokenKind;
pub use tokenizer::Tokenizer;

/// Convenience function to parse and analyze a command in one step.
pub fn parse_and_analyze(source: &str) -> (ParsedShell, security::SecurityAnalysis) {
    let mut parser = ShellParser::new();
    let cmd = parser.parse(source);
    let analysis = security::analyze(&cmd);
    (cmd, analysis)
}

/// Parse and analyze reusing an existing parser instance.
///
/// Avoids re-creating the tree-sitter parser each call, useful in hot paths
/// that analyze many commands in sequence.
pub fn parse_and_analyze_with(
    parser: &mut ShellParser,
    source: &str,
) -> (ParsedShell, security::SecurityAnalysis) {
    let cmd = parser.parse(source);
    let analysis = security::analyze(&cmd);
    (cmd, analysis)
}

/// Convenience function to check if a command string is safe.
///
/// A command is considered safe if:
/// - It can be parsed as a "word-only" command sequence
/// - It has no high or critical security risks
pub fn is_safe_command(source: &str) -> bool {
    let mut parser = ShellParser::new();
    let cmd = parser.parse(source);

    // Must be a word-only command sequence
    if cmd.try_extract_safe_commands().is_none() {
        return false;
    }

    // Check security analysis
    let analysis = security::analyze(&cmd);
    !analysis.requires_approval()
        && analysis
            .risks
            .iter()
            .all(|r| r.level < security::RiskLevel::High)
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
