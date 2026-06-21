//! Destructive command warning patterns.
//!
//! This is **purely informational** — it does NOT affect permission logic or
//! auto-approval. The returned note is meant for display in the
//! permission-request UI; the BashTool never denies a command on the basis of
//! these patterns (the warning rides behind a default-off feature flag).
//!
//! Patterns are word-boundary regexes. The `regex` crate has no
//! negative-lookahead, so the one rule that uses one (`git clean`
//! force-but-not-dry-run) is expressed as a force-match minus a dry-run match.

use std::sync::LazyLock;

use regex::Regex;

/// A destructive-command rule. Most are a single regex; `git clean` needs an
/// extra dry-run exclusion the `regex` crate can't express as a lookahead.
enum Rule {
    Regex(Regex),
    GitCleanForce { force: Regex, dry_run: Regex },
}

impl Rule {
    fn matches(&self, command: &str) -> bool {
        match self {
            Rule::Regex(re) => re.is_match(command),
            Rule::GitCleanForce { force, dry_run } => {
                force.is_match(command) && !dry_run.is_match(command)
            }
        }
    }
}

/// Ordered destructive-command rules (first match wins). Warnings are
/// informational strings surfaced in the permission-request UI.
#[allow(clippy::expect_used)] // static init of compile-time-constant patterns
static RULES: LazyLock<Vec<(Rule, &'static str)>> = LazyLock::new(|| {
    let re = |p: &str| Regex::new(p).expect("valid destructive pattern");
    vec![
        // Git — data loss / hard to reverse
        (
            Rule::Regex(re(r"\bgit\s+reset\s+--hard\b")),
            "Note: may discard uncommitted changes",
        ),
        (
            Rule::Regex(re(
                r"\bgit\s+push\b[^;&|\n]*[ \t](--force|--force-with-lease|-f)\b",
            )),
            "Note: may overwrite remote history",
        ),
        (
            Rule::GitCleanForce {
                force: re(r"\bgit\s+clean\b[^;&|\n]*-[a-zA-Z]*f"),
                dry_run: re(r"\bgit\s+clean\b[^;&|\n]*(--dry-run|-[a-zA-Z]*n)"),
            },
            "Note: may permanently delete untracked files",
        ),
        (
            Rule::Regex(re(r"\bgit\s+checkout\s+(--\s+)?\.[ \t]*($|[;&|\n])")),
            "Note: may discard all working tree changes",
        ),
        (
            Rule::Regex(re(r"\bgit\s+restore\s+(--\s+)?\.[ \t]*($|[;&|\n])")),
            "Note: may discard all working tree changes",
        ),
        (
            Rule::Regex(re(r"\bgit\s+stash[ \t]+(drop|clear)\b")),
            "Note: may permanently remove stashed changes",
        ),
        (
            Rule::Regex(re(
                r"\bgit\s+branch\s+(-D[ \t]|--delete\s+--force|--force\s+--delete)\b",
            )),
            "Note: may force-delete a branch",
        ),
        // Git — safety bypass
        (
            Rule::Regex(re(r"\bgit\s+(commit|push|merge)\b[^;&|\n]*--no-verify\b")),
            "Note: may skip safety hooks",
        ),
        (
            Rule::Regex(re(r"\bgit\s+commit\b[^;&|\n]*--amend\b")),
            "Note: may rewrite the last commit",
        ),
        // File deletion (dangerous absolute paths handled elsewhere)
        (
            Rule::Regex(re(
                r"(^|[;&|\n]\s*)rm\s+-[a-zA-Z]*[rR][a-zA-Z]*f|(^|[;&|\n]\s*)rm\s+-[a-zA-Z]*f[a-zA-Z]*[rR]",
            )),
            "Note: may recursively force-remove files",
        ),
        (
            Rule::Regex(re(r"(^|[;&|\n]\s*)rm\s+-[a-zA-Z]*[rR]")),
            "Note: may recursively remove files",
        ),
        (
            Rule::Regex(re(r"(^|[;&|\n]\s*)rm\s+-[a-zA-Z]*f")),
            "Note: may force-remove files",
        ),
        // Database
        (
            Rule::Regex(re(r"(?i)\b(DROP|TRUNCATE)\s+(TABLE|DATABASE|SCHEMA)\b")),
            "Note: may drop or truncate database objects",
        ),
        (
            Rule::Regex(re(r#"(?i)\bDELETE\s+FROM\s+\w+[ \t]*(;|"|'|\n|$)"#)),
            "Note: may delete all rows from a database table",
        ),
        // Infrastructure
        (
            Rule::Regex(re(r"\bkubectl\s+delete\b")),
            "Note: may delete Kubernetes resources",
        ),
        (
            Rule::Regex(re(r"\bterraform\s+destroy\b")),
            "Note: may destroy Terraform infrastructure",
        ),
    ]
});

/// Return an informational warning if `command` matches a destructive pattern.
///
/// Advisory only — callers MUST NOT use this to block or deny a command.
pub fn get_destructive_warning(command: &str) -> Option<String> {
    RULES
        .iter()
        .find(|(rule, _)| rule.matches(command))
        .map(|(_, warning)| (*warning).to_string())
}

#[cfg(test)]
#[path = "destructive.test.rs"]
mod tests;
