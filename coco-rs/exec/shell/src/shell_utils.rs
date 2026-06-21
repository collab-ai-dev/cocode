//! Shell utility functions: quoting, escaping, command splitting, shell
//! detection, and working directory tracking helpers.

use crate::tokenizer::TokenKind;
use crate::tokenizer::tokenize;

// ── Shell quoting and escaping ──

/// Quote a single argument for safe use in a bash command.
///
/// The argument is wrapped in single quotes. Any embedded single quotes
/// are escaped using the `'\''` idiom (end quote, escaped quote, start quote).
pub fn quote_arg(arg: &str) -> String {
    if arg.is_empty() {
        return "''".to_string();
    }

    // If the arg contains no special characters, return it unquoted
    if arg
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'/' | b':' | b'@'))
    {
        return arg.to_string();
    }

    // Wrap in single quotes, escaping embedded single quotes
    let mut out = String::with_capacity(arg.len() + 4);
    out.push('\'');
    for c in arg.chars() {
        if c == '\'' {
            out.push_str("'\"'\"'");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// Escape a string for safe embedding inside double quotes in bash.
///
/// Escapes characters that have special meaning inside double quotes:
/// `$`, `` ` ``, `"`, `\`, `!`.
pub fn escape_for_double_quotes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '$' | '`' | '"' | '\\' | '!' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// Escape a string for use as an unquoted bash argument.
///
/// Backslash-escapes all shell metacharacters.
pub fn escape_for_bash(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if is_shell_metachar(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn is_shell_metachar(c: char) -> bool {
    matches!(
        c,
        ' ' | '\t'
            | '\n'
            | '|'
            | '&'
            | ';'
            | '('
            | ')'
            | '<'
            | '>'
            | '"'
            | '\''
            | '`'
            | '$'
            | '\\'
            | '!'
            | '{'
            | '}'
            | '['
            | ']'
            | '*'
            | '?'
            | '~'
            | '#'
    )
}

/// Quote a shell command for safe use with `eval` or as a single argument.
///
/// If the command contains heredocs or multiline strings, uses single-quote
/// wrapping with proper escaping. Otherwise uses `quote_arg`.
pub fn quote_shell_command(command: &str, add_stdin_redirect: bool) -> String {
    if contains_heredoc(command) || contains_multiline_string(command) {
        let escaped = command.replace('\'', "'\"'\"'");
        let quoted = format!("'{escaped}'");

        // Don't add stdin redirect for heredocs
        if contains_heredoc(command) {
            return quoted;
        }

        if add_stdin_redirect {
            format!("{quoted} < /dev/null")
        } else {
            quoted
        }
    } else if add_stdin_redirect {
        format!("{} < /dev/null", quote_arg(command))
    } else {
        quote_arg(command)
    }
}

/// Detect if a command already has a stdin redirect.
///
/// Matches `< file` but not `<<` (heredoc) or `<(` (process substitution).
pub fn has_stdin_redirect(command: &str) -> bool {
    let bytes = command.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            // Check it's not << or <(
            if i + 1 < bytes.len() && (bytes[i + 1] == b'<' || bytes[i + 1] == b'(') {
                i += 2;
                continue;
            }
            // Check it's preceded by whitespace, separator, or start
            if i == 0 || matches!(bytes[i - 1], b' ' | b'\t' | b';' | b'&' | b'|') {
                // Check it's followed by whitespace then a non-empty target
                let mut j = i + 1;
                while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] != b'\n' {
                    return true;
                }
            }
        }
        i += 1;
    }
    false
}

/// Check if stdin redirect should be added to a command.
pub fn should_add_stdin_redirect(command: &str) -> bool {
    if contains_heredoc(command) {
        return false;
    }
    if has_stdin_redirect(command) {
        return false;
    }
    true
}

/// Rewrite Windows CMD-style `>nul` redirects to POSIX `/dev/null`.
///
/// Handles `>nul`, `> NUL`, `2>nul`, `&>nul`, `>>nul` (case-insensitive).
/// Does not match `>null`, `>nullable`, `>nul.txt`.
pub fn rewrite_windows_null_redirect(command: &str) -> String {
    let bytes = command.as_bytes();
    let mut out = String::with_capacity(command.len());
    let mut i = 0;

    while i < bytes.len() {
        // Look for redirect operators followed by "nul"
        if bytes[i] == b'>' || (bytes[i] == b'&' && i + 1 < bytes.len() && bytes[i + 1] == b'>') {
            let redir_start = i;
            // Skip past the redirect operator
            if bytes[i] == b'&' {
                i += 1; // &
            }
            if i < bytes.len() && bytes[i] == b'>' {
                i += 1;
                if i < bytes.len() && bytes[i] == b'>' {
                    i += 1; // >>
                }
            }
            // Skip optional whitespace
            while i < bytes.len() && bytes[i] == b' ' {
                i += 1;
            }
            // Check for "nul" (case-insensitive) not followed by identifier chars
            if i + 2 < bytes.len()
                && bytes[i].eq_ignore_ascii_case(&b'n')
                && bytes[i + 1].eq_ignore_ascii_case(&b'u')
                && bytes[i + 2].eq_ignore_ascii_case(&b'l')
            {
                let after = if i + 3 < bytes.len() {
                    bytes[i + 3]
                } else {
                    b'\n'
                };
                if matches!(after, b' ' | b'\t' | b'\n' | b'|' | b'&' | b';' | b')')
                    || i + 3 >= bytes.len()
                {
                    // It's >nul — rewrite
                    out.push_str(&command[redir_start..i]);
                    out.push_str("/dev/null");
                    i += 3;
                    continue;
                }
            }
            // Not a nul redirect, push everything we consumed
            out.push_str(&command[redir_start..i]);
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }

    out
}

// ── Command splitting ──

/// A segment of a compound command after splitting on separators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSegment {
    /// The command text.
    pub command: String,
    /// The separator that follows this segment (if any).
    pub separator: Option<String>,
}

/// Split a command string into segments separated by unquoted `;`, `&&`, `||`, or `|`.
///
/// Respects quoting and parenthesis nesting.
pub fn split_command_segments(command: &str) -> Vec<CommandSegment> {
    let tokens = tokenize(command);
    let mut segments = Vec::new();
    let mut current = String::new();

    for token in &tokens {
        match token.kind {
            TokenKind::Operator
                if matches!(token.value.as_str(), ";" | "&&" | "||" | "|" | "|&" | "&") =>
            {
                let cmd = current.trim().to_string();
                if !cmd.is_empty() {
                    segments.push(CommandSegment {
                        command: cmd,
                        separator: Some(token.value.clone()),
                    });
                }
                current.clear();
            }
            TokenKind::Eof => break,
            _ => {
                if !current.is_empty() && !token.value.starts_with('\n') {
                    current.push(' ');
                }
                current.push_str(&token.value);
            }
        }
    }

    let remaining = current.trim().to_string();
    if !remaining.is_empty() {
        segments.push(CommandSegment {
            command: remaining,
            separator: None,
        });
    }

    segments
}

/// Extract just the first command from a compound command string.
///
/// Useful for identifying what program is being invoked.
pub fn first_command(input: &str) -> Option<String> {
    let segments = split_command_segments(input);
    segments.into_iter().next().map(|s| s.command)
}

// ── Shell detection ──

/// Known shell types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    Bash,
    Zsh,
    Fish,
    Sh,
    Dash,
    Unknown,
}

/// Detect the shell type from a shell path string.
///
/// Inspects the basename of the path to determine the shell variant.
pub fn detect_shell(shell_path: &str) -> ShellKind {
    let basename = shell_path
        .rsplit('/')
        .next()
        .unwrap_or(shell_path)
        .to_lowercase();

    if basename.contains("bash") {
        ShellKind::Bash
    } else if basename.contains("zsh") {
        ShellKind::Zsh
    } else if basename.contains("fish") {
        ShellKind::Fish
    } else if basename == "dash" {
        ShellKind::Dash
    } else if basename == "sh" {
        ShellKind::Sh
    } else {
        ShellKind::Unknown
    }
}

/// Get the command to disable extended glob patterns for a given shell.
///
/// Extended globs can be exploited via malicious filenames that expand
/// after security validation.
pub fn disable_extglob_command(shell: ShellKind) -> Option<&'static str> {
    match shell {
        ShellKind::Bash => Some("shopt -u extglob 2>/dev/null || true"),
        ShellKind::Zsh => Some("setopt NO_EXTENDED_GLOB 2>/dev/null || true"),
        _ => None,
    }
}

// ── Heredoc and multiline detection ──

/// Check if a command contains a heredoc pattern (<<DELIM).
fn contains_heredoc(command: &str) -> bool {
    let bytes = command.as_bytes();
    let mut i = 0;

    while i + 1 < bytes.len() {
        if bytes[i] == b'<' && bytes[i + 1] == b'<' {
            // Skip <<< (herestring)
            if i + 2 < bytes.len() && bytes[i + 2] == b'<' {
                i += 3;
                continue;
            }
            // Not inside arithmetic: check for digit << digit pattern
            if i > 0
                && bytes[i - 1].is_ascii_digit()
                && i + 2 < bytes.len()
                && bytes[i + 2].is_ascii_digit()
            {
                i += 2;
                continue;
            }
            return true;
        }
        i += 1;
    }
    false
}

/// Check if a command contains multiline strings in quotes.
fn contains_multiline_string(command: &str) -> bool {
    let bytes = command.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        match bytes[i] {
            b'\'' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'\'' {
                    if bytes[i] == b'\n' {
                        return true;
                    }
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\n' {
                        return true;
                    }
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        i += 1;
                    }
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    false
}

// ── Working directory tracking ──

/// CWD marker delimiters used to track working directory changes in shell output.
pub const CWD_MARKER_PREFIX: &str = "__COCO_CWD__:";
pub const CWD_MARKER_SUFFIX: &str = ":__COCO_CWD_END__";

/// Generate a shell command that prints the current working directory
/// wrapped in CWD tracking markers.
pub fn cwd_tracking_command() -> String {
    format!("echo '{CWD_MARKER_PREFIX}'\"$PWD\"'{CWD_MARKER_SUFFIX}'")
}

/// Extract a CWD path from a line of shell output containing CWD markers.
///
/// Returns `None` if the line doesn't contain valid CWD markers.
pub fn extract_cwd_from_output(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let after_prefix = trimmed.strip_prefix(CWD_MARKER_PREFIX)?;
    after_prefix.strip_suffix(CWD_MARKER_SUFFIX)
}

#[cfg(test)]
#[path = "shell_utils.test.rs"]
mod tests;
