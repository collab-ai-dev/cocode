/// Result of a command safety analysis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SafetyResult {
    /// Command is safe to execute without permission.
    Safe,
    /// Command requires user approval.
    RequiresApproval { reason: String },
    /// Command is denied (destructive or dangerous).
    Denied { reason: String },
}

impl SafetyResult {
    pub fn is_safe(&self) -> bool {
        matches!(self, Self::Safe)
    }

    pub fn is_denied(&self) -> bool {
        matches!(self, Self::Denied { .. })
    }
}

/// Security check identifiers (23 check types).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecurityCheckId(pub i32);

impl SecurityCheckId {
    pub const INCOMPLETE_COMMANDS: Self = Self(1);
    pub const JQ_SYSTEM_FUNCTION: Self = Self(2);
    pub const OBFUSCATED_FLAGS: Self = Self(3);
    pub const SHELL_METACHARACTERS: Self = Self(4);
    pub const DANGEROUS_VARIABLES: Self = Self(5);
    pub const NEWLINES: Self = Self(6);
    pub const DANGEROUS_PATTERNS_SUBSHELL: Self = Self(7);
    pub const DANGEROUS_PATTERNS_REDIRECTION: Self = Self(8);
    pub const DANGEROUS_PATTERNS_CONDITIONAL: Self = Self(9);
    pub const IFS_INJECTION: Self = Self(10);
    pub const GIT_COMMIT_SUBSTITUTION: Self = Self(11);
    pub const PROC_ENVIRON_ACCESS: Self = Self(12);
    pub const MALFORMED_TOKEN_INJECTION: Self = Self(13);
    pub const BACKSLASH_ESCAPED_WHITESPACE: Self = Self(14);
    pub const BRACE_EXPANSION: Self = Self(15);
    pub const CONTROL_CHARACTERS: Self = Self(16);
    pub const UNICODE_WHITESPACE: Self = Self(17);
    pub const MID_WORD_HASH: Self = Self(18);
    pub const ZSH_DANGEROUS_COMMANDS: Self = Self(19);
    pub const BACKSLASH_ESCAPED_OPERATORS: Self = Self(20);
    pub const COMMENT_QUOTE_DESYNC: Self = Self(21);
    pub const QUOTED_NEWLINE: Self = Self(22);
    pub const DANGEROUS_PATTERNS_GENERAL: Self = Self(23);
}
