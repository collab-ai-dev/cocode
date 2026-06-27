//! Security risk type definitions.

use std::fmt;

/// Risk severity levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RiskLevel {
    /// Low risk - may be intentional, minor impact.
    Low,
    /// Medium risk - potentially dangerous, requires attention.
    Medium,
    /// High risk - likely dangerous, should be reviewed carefully.
    High,
    /// Critical risk - almost certainly dangerous, should be blocked.
    Critical,
}

impl fmt::Display for RiskLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RiskLevel::Low => write!(f, "low"),
            RiskLevel::Medium => write!(f, "medium"),
            RiskLevel::High => write!(f, "high"),
            RiskLevel::Critical => write!(f, "critical"),
        }
    }
}

/// The phase at which a risk is evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RiskPhase {
    /// Deny phase - high-confidence injection patterns auto-denied by consumers.
    Deny,
    /// Ask phase - risks that require user approval.
    Ask,
}

impl fmt::Display for RiskPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RiskPhase::Deny => write!(f, "deny"),
            RiskPhase::Ask => write!(f, "ask"),
        }
    }
}

/// Specific types of security risks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RiskKind {
    // Deny phase risks (16)
    /// Single-quote bypass via backslash at end of single-quoted string.
    SingleQuoteBypass,
    /// Dangerous jq operations (system() calls).
    JqDanger,
    /// Obfuscated flags using $'...' or $"..." syntax.
    ObfuscatedFlags,
    /// Shell metacharacters in command arguments.
    ShellMetacharacters,
    /// Dangerous variable patterns ($VAR |, ${VAR} |).
    DangerousVariables,
    /// Newline injection (\n followed by command).
    NewlineInjection,
    /// IFS manipulation.
    IfsInjection,
    /// Access to /proc/*/environ.
    ProcEnvironAccess,
    /// Backslash before whitespace outside quotes (line-continuation injection).
    BackslashEscapedWhitespace,
    /// Backslash before shell operators outside quotes (\;, \|, \&, etc.).
    BackslashEscapedOperators,
    /// Non-ASCII Unicode whitespace characters (zero-width spaces, NBSP, etc.).
    UnicodeWhitespace,
    /// Hash character not preceded by whitespace (potential comment injection).
    MidWordHash,
    /// Brace expansion ({a,b} or {1..3}) outside quotes.
    BraceExpansion,
    /// Dangerous zsh-specific commands (zmodload, emulate, etc.).
    ZshDangerousCommands,
    /// Odd number of unescaped quotes after # (comment/quote desync).
    CommentQuoteDesync,
    /// Literal newline followed by # inside double quotes.
    QuotedNewlineHash,
    /// Excess unbalanced closing braces/brackets after quote stripping.
    ExcessClosingBraces,
    /// Eval-like builtins that execute their arguments as shell code.
    EvalLikeBuiltin,
    /// Builtins whose NAME arguments trigger array subscript evaluation.
    SubscriptEval,
    /// Arithmetic comparisons in `[[` that evaluate subscripts.
    ArithComparison,

    // Ask phase risks (9)
    /// Unsafe heredoc in command substitution (unquoted delimiter allows expansion).
    UnsafeHeredocSubstitution,
    /// Dangerous substitutions ($(), ${}, <(), etc.).
    DangerousSubstitution,
    /// Malformed tokens (unbalanced brackets, quotes).
    MalformedTokens,
    /// Sensitive file redirections.
    SensitiveRedirect,
    /// Network exfiltration attempts.
    NetworkExfiltration,
    /// Privilege escalation attempts.
    PrivilegeEscalation,
    /// File system tampering (rm -rf, chmod, etc.).
    FileSystemTampering,
    /// Code execution risks (eval, exec, etc.).
    CodeExecution,
    /// Dangerous operations targeting sensitive system paths.
    DangerousPath,
}

impl RiskKind {
    /// Returns the default risk level for this kind.
    pub fn default_level(&self) -> RiskLevel {
        match self {
            // Deny phase
            RiskKind::SingleQuoteBypass => RiskLevel::Critical,
            RiskKind::JqDanger => RiskLevel::High,
            RiskKind::ObfuscatedFlags => RiskLevel::Medium,
            RiskKind::ShellMetacharacters => RiskLevel::Medium,
            RiskKind::DangerousVariables => RiskLevel::Medium,
            RiskKind::NewlineInjection => RiskLevel::High,
            RiskKind::IfsInjection => RiskLevel::High,
            RiskKind::ProcEnvironAccess => RiskLevel::High,
            RiskKind::BackslashEscapedWhitespace => RiskLevel::Medium,
            RiskKind::BackslashEscapedOperators => RiskLevel::Medium,
            RiskKind::UnicodeWhitespace => RiskLevel::High,
            RiskKind::MidWordHash => RiskLevel::Medium,
            RiskKind::BraceExpansion => RiskLevel::Medium,
            RiskKind::ZshDangerousCommands => RiskLevel::High,
            RiskKind::CommentQuoteDesync => RiskLevel::Medium,
            RiskKind::QuotedNewlineHash => RiskLevel::Medium,
            RiskKind::ExcessClosingBraces => RiskLevel::Medium,
            RiskKind::EvalLikeBuiltin => RiskLevel::High,
            RiskKind::SubscriptEval => RiskLevel::High,
            RiskKind::ArithComparison => RiskLevel::High,

            // Ask phase - generally higher severity
            RiskKind::UnsafeHeredocSubstitution => RiskLevel::Medium,
            RiskKind::DangerousSubstitution => RiskLevel::Medium,
            RiskKind::MalformedTokens => RiskLevel::Low,
            RiskKind::SensitiveRedirect => RiskLevel::High,
            RiskKind::NetworkExfiltration => RiskLevel::Critical,
            RiskKind::PrivilegeEscalation => RiskLevel::Critical,
            RiskKind::FileSystemTampering => RiskLevel::High,
            RiskKind::CodeExecution => RiskLevel::Critical,
            RiskKind::DangerousPath => RiskLevel::High,
        }
    }

    /// Returns the phase for this risk kind.
    pub fn phase(&self) -> RiskPhase {
        match self {
            RiskKind::SingleQuoteBypass
            | RiskKind::JqDanger
            | RiskKind::ObfuscatedFlags
            | RiskKind::ShellMetacharacters
            | RiskKind::DangerousVariables
            | RiskKind::NewlineInjection
            | RiskKind::IfsInjection
            | RiskKind::ProcEnvironAccess
            | RiskKind::BackslashEscapedWhitespace
            | RiskKind::BackslashEscapedOperators
            | RiskKind::UnicodeWhitespace
            | RiskKind::MidWordHash
            | RiskKind::BraceExpansion
            | RiskKind::ZshDangerousCommands
            | RiskKind::CommentQuoteDesync
            | RiskKind::QuotedNewlineHash
            | RiskKind::ExcessClosingBraces
            | RiskKind::EvalLikeBuiltin
            | RiskKind::SubscriptEval
            | RiskKind::ArithComparison => RiskPhase::Deny,

            RiskKind::UnsafeHeredocSubstitution
            | RiskKind::DangerousSubstitution
            | RiskKind::MalformedTokens
            | RiskKind::SensitiveRedirect
            | RiskKind::NetworkExfiltration
            | RiskKind::PrivilegeEscalation
            | RiskKind::FileSystemTampering
            | RiskKind::CodeExecution
            | RiskKind::DangerousPath => RiskPhase::Ask,
        }
    }

    /// Returns a human-readable name for this risk kind.
    pub fn name(&self) -> &'static str {
        match self {
            RiskKind::SingleQuoteBypass => "single quote bypass",
            RiskKind::JqDanger => "jq danger",
            RiskKind::ObfuscatedFlags => "obfuscated flags",
            RiskKind::ShellMetacharacters => "shell metacharacters",
            RiskKind::DangerousVariables => "dangerous variables",
            RiskKind::NewlineInjection => "newline injection",
            RiskKind::IfsInjection => "IFS injection",
            RiskKind::ProcEnvironAccess => "/proc environ access",
            RiskKind::BackslashEscapedWhitespace => "backslash escaped whitespace",
            RiskKind::BackslashEscapedOperators => "backslash escaped operators",
            RiskKind::UnicodeWhitespace => "unicode whitespace",
            RiskKind::MidWordHash => "mid-word hash",
            RiskKind::BraceExpansion => "brace expansion",
            RiskKind::ZshDangerousCommands => "zsh dangerous commands",
            RiskKind::CommentQuoteDesync => "comment quote desync",
            RiskKind::QuotedNewlineHash => "quoted newline hash",
            RiskKind::UnsafeHeredocSubstitution => "unsafe heredoc substitution",
            RiskKind::DangerousSubstitution => "dangerous substitution",
            RiskKind::MalformedTokens => "malformed tokens",
            RiskKind::SensitiveRedirect => "sensitive redirect",
            RiskKind::NetworkExfiltration => "network exfiltration",
            RiskKind::PrivilegeEscalation => "privilege escalation",
            RiskKind::FileSystemTampering => "file system tampering",
            RiskKind::CodeExecution => "code execution",
            RiskKind::DangerousPath => "dangerous path",
            RiskKind::ExcessClosingBraces => "excess closing braces",
            RiskKind::EvalLikeBuiltin => "eval-like builtin",
            RiskKind::SubscriptEval => "subscript evaluation",
            RiskKind::ArithComparison => "arithmetic comparison evaluation",
        }
    }
}

impl fmt::Display for RiskKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// A detected security risk.
#[derive(Debug, Clone)]
pub struct SecurityRisk {
    /// The type of risk.
    pub kind: RiskKind,
    /// The severity level.
    pub level: RiskLevel,
    /// The evaluation phase.
    pub phase: RiskPhase,
    /// Human-readable description of the risk.
    pub message: String,
    /// The span in the source where the risk was detected.
    pub span: Option<crate::tokenizer::Span>,
    /// The specific text that triggered the risk.
    pub matched_text: Option<String>,
}

impl SecurityRisk {
    /// Create a new security risk.
    pub fn new(kind: RiskKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            level: kind.default_level(),
            phase: kind.phase(),
            message: message.into(),
            span: None,
            matched_text: None,
        }
    }

    /// Set a custom risk level.
    pub fn with_level(mut self, level: RiskLevel) -> Self {
        self.level = level;
        self
    }

    /// Set the span where the risk was detected.
    pub fn with_span(mut self, span: crate::tokenizer::Span) -> Self {
        self.span = Some(span);
        self
    }

    /// Set the text that triggered the risk.
    pub fn with_matched_text(mut self, text: impl Into<String>) -> Self {
        self.matched_text = Some(text.into());
        self
    }
}

impl fmt::Display for SecurityRisk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}: {}", self.level, self.kind, self.message)
    }
}

/// Result of security analysis.
#[derive(Debug, Clone, Default)]
pub struct SecurityAnalysis {
    /// All detected risks.
    pub risks: Vec<SecurityRisk>,
    /// The highest risk level found.
    pub max_level: Option<RiskLevel>,
}

impl SecurityAnalysis {
    /// Create a new empty analysis.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a risk to the analysis.
    pub fn add_risk(&mut self, risk: SecurityRisk) {
        // Update max level
        if self.max_level.is_none_or(|l| risk.level > l) {
            self.max_level = Some(risk.level);
        }
        self.risks.push(risk);
    }

    /// Returns true if any risks were detected.
    pub fn has_risks(&self) -> bool {
        !self.risks.is_empty()
    }

    /// Returns the number of risks detected.
    pub fn risk_count(&self) -> usize {
        self.risks.len()
    }

    /// Returns risks filtered by phase.
    pub fn risks_by_phase(&self, phase: RiskPhase) -> Vec<&SecurityRisk> {
        self.risks.iter().filter(|r| r.phase == phase).collect()
    }

    /// Returns risks filtered by minimum level.
    pub fn risks_at_or_above(&self, level: RiskLevel) -> Vec<&SecurityRisk> {
        self.risks.iter().filter(|r| r.level >= level).collect()
    }

    /// Returns true if any risk is in the Deny phase (auto-denied by consumers).
    pub fn is_auto_denied(&self) -> bool {
        self.risks.iter().any(|r| r.phase == RiskPhase::Deny)
    }

    /// Returns true if any risk requires user approval (Ask phase).
    pub fn requires_approval(&self) -> bool {
        self.risks.iter().any(|r| r.phase == RiskPhase::Ask)
    }

    /// Merge another analysis into this one.
    pub fn merge(&mut self, other: SecurityAnalysis) {
        for risk in other.risks {
            self.add_risk(risk);
        }
    }
}

#[cfg(test)]
#[path = "risks.test.rs"]
mod tests;
