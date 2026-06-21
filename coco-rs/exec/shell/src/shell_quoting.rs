//! Shell-safe quoting utilities.
//!
//! Does not depend on Node's `shell-quote` package.
//! For our use case (wrapping a user command for `eval`), single-quoting the
//! entire command is both simpler and avoids every edge case that the TS
//! `shell-quote` library introduces (the `!` → `\!` corruption, the `\'` bug
//! tracked in HackerOne #3482049, the curly-brace token misparsing). The
//! escape sequence `'"'"'` (close-single, literal-single-in-double, reopen)
//! is the POSIX-portable way to escape `'` inside single-quoted strings.

/// Single-quote a string for use as an `eval` argument.
///
/// Escapes embedded single quotes via the canonical `'"'"'` trick so the
/// output is safe to pass through bash's word-splitting and quote-parsing.
pub fn single_quote_for_eval(s: &str) -> String {
    let escaped = s.replace('\'', r#"'"'"'"#);
    format!("'{escaped}'")
}

/// Quote a sequence of arguments into a single shell-safe string.
///
/// Each argument is single-quoted and joined with spaces.
pub fn quote<S: AsRef<str>>(args: &[S]) -> String {
    args.iter()
        .map(|s| single_quote_for_eval(s.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Quote arguments the way the npm `shell-quote` package's `quote()` does:
/// quote/escape **only when a token needs it** (a bare token is returned
/// verbatim). Use this to render model-facing synthetic command strings (e.g.
/// the `ls <dir>` narration for an `@`-mentioned directory) byte-for-byte like
/// the upstream TS implementation. For wrapping a *real* command for `eval`,
/// use [`quote`] (which always single-quotes).
pub fn quote_posix<S: AsRef<str>>(args: &[S]) -> String {
    args.iter()
        .map(|s| quote_posix_arg(s.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Per-argument logic mirroring `shell-quote`'s three branches.
fn quote_posix_arg(s: &str) -> String {
    let has_double = s.contains('"');
    let has_single = s.contains('\'');
    let has_ws = s.chars().any(char::is_whitespace);

    if (has_double || has_ws) && !has_single {
        // Single-quote; escape `'` and `\` (no `'` present in this branch).
        let escaped = s.replace('\\', "\\\\").replace('\'', "\\'");
        format!("'{escaped}'")
    } else if has_double || has_single || has_ws {
        // Double-quote; escape `"` `\` `$` `` ` `` `!`.
        let mut out = String::with_capacity(s.len() + 2);
        out.push('"');
        for ch in s.chars() {
            if matches!(ch, '"' | '\\' | '$' | '`' | '!') {
                out.push('\\');
            }
            out.push(ch);
        }
        out.push('"');
        out
    } else {
        // No whitespace/quotes: backslash-escape individual shell metachars.
        // Bare paths (alphanumerics, `/`, `-`, `_`, `.`) pass through untouched.
        let mut out = String::with_capacity(s.len());
        for ch in s.chars() {
            if is_posix_metachar(ch) {
                out.push('\\');
            }
            out.push(ch);
        }
        out
    }
}

/// The metacharacter set `shell-quote` backslash-escapes in unquoted tokens.
fn is_posix_metachar(ch: char) -> bool {
    matches!(
        ch,
        '#' | '!'
            | '"'
            | '$'
            | '&'
            | '\''
            | '('
            | ')'
            | '*'
            | ','
            | ':'
            | ';'
            | '<'
            | '='
            | '>'
            | '?'
            | '@'
            | '['
            | '\\'
            | ']'
            | '^'
            | '`'
            | '{'
            | '|'
            | '}'
    )
}

/// Detect a heredoc pattern in the command.
///
/// Matches `<<EOF`, `<<'EOF'`, `<<"EOF"`, `<<-EOF`, `<<-'EOF'`, `<<\EOF`.
/// Excludes bit-shift operators (`1 << 2`, `[[ 1 << 2 ]]`, `$(( 1 << 2 ))`).
pub fn contains_heredoc(command: &str) -> bool {
    // Bit-shift exclusions (Rust regex doesn't backtrack, so we keep these as
    // simple substring/digit walks — cheaper than re-compiling regexes).
    if has_bit_shift(command) {
        return false;
    }
    // `<<` followed by optional `-`, optional space, then either:
    //   - a quoted word: `'WORD'` / `"WORD"`
    //   - a backslash-escaped word: `\WORD`
    //   - a bare word
    let bytes = command.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'<' && bytes[i + 1] == b'<' {
            let mut j = i + 2;
            // Optional `-`
            if j < bytes.len() && bytes[j] == b'-' {
                j += 1;
            }
            // Optional whitespace
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                j += 1;
            }
            // Quote, backslash, or word char
            if j < bytes.len() {
                let c = bytes[j];
                if c == b'\'' || c == b'"' || c == b'\\' {
                    let rest = &command[j + 1..];
                    if rest.chars().next().is_some_and(is_word_char) {
                        return true;
                    }
                } else if is_word_char(c as char) {
                    return true;
                }
            }
        }
        i += 1;
    }
    false
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn has_bit_shift(command: &str) -> bool {
    let bytes = command.as_bytes();
    let mut i = 0;
    while i + 2 < bytes.len() {
        if bytes[i].is_ascii_digit() {
            // Skip the digit sequence
            let mut j = i;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            // Skip optional whitespace
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                j += 1;
            }
            if j + 1 < bytes.len() && bytes[j] == b'<' && bytes[j + 1] == b'<' {
                // Skip `<<` + optional whitespace
                let mut k = j + 2;
                while k < bytes.len() && (bytes[k] == b' ' || bytes[k] == b'\t') {
                    k += 1;
                }
                if k < bytes.len() && bytes[k].is_ascii_digit() {
                    return true;
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    false
}

/// Detect multiline strings inside single or double quotes.
pub fn contains_multiline_string(command: &str) -> bool {
    contains_multiline_in_quotes(command, '\'') || contains_multiline_in_quotes(command, '"')
}

fn contains_multiline_in_quotes(s: &str, q: char) -> bool {
    let bytes = s.as_bytes();
    let q_byte = q as u8;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == q_byte {
            let mut j = i + 1;
            let mut saw_newline = false;
            while j < bytes.len() {
                if bytes[j] == b'\\' && j + 1 < bytes.len() {
                    j += 2;
                    continue;
                }
                if bytes[j] == b'\n' {
                    saw_newline = true;
                }
                if bytes[j] == q_byte {
                    if saw_newline {
                        return true;
                    }
                    break;
                }
                j += 1;
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    false
}

/// Detect an existing stdin redirect.
///
/// Matches `< file`, `< /path`, `< /dev/null`. Excludes `<<` (heredoc) and
/// `<(` (process substitution). Must be preceded by whitespace or a command
/// separator (or start of string).
pub fn has_stdin_redirect(command: &str) -> bool {
    let bytes = command.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            let prev_ok = i == 0
                || matches!(
                    bytes[i - 1],
                    b' ' | b'\t' | b';' | b'&' | b'|' | b'\n' | b'\r'
                );
            let next = bytes.get(i + 1).copied();
            let not_heredoc_or_procsub = !matches!(next, Some(b'<') | Some(b'('));
            if prev_ok && not_heredoc_or_procsub {
                // Skip whitespace, then require at least one non-whitespace.
                let mut j = i + 1;
                while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                    j += 1;
                }
                if j < bytes.len() && !bytes[j].is_ascii_whitespace() {
                    return true;
                }
            }
        }
        i += 1;
    }
    false
}

/// Decide whether to inject a `< /dev/null` stdin redirect when wrapping a
/// command for `eval`.
///
/// - Heredocs already provide stdin → skip.
/// - Commands with an existing redirect → skip.
/// - Otherwise add (default safe behavior — prevents the child from blocking
///   on the inherited pipe stdin).
pub fn should_add_stdin_redirect(command: &str) -> bool {
    !contains_heredoc(command) && !has_stdin_redirect(command)
}

/// Wrap a command for `eval`, preserving heredocs and multiline strings.
///
/// Returns a string suitable as the argument to `eval` (already single-quoted).
/// When `add_stdin_redirect` is true and the command has no heredoc, appends
/// `< /dev/null` *outside* the quote so the redirect applies to `eval` itself
/// — this is critical for piped commands (see [`crate::pipe_rearrange`]).
pub fn quote_shell_command(command: &str, add_stdin_redirect: bool) -> String {
    let quoted = single_quote_for_eval(command);
    if contains_heredoc(command) {
        // Heredocs provide their own stdin — never add a redirect.
        return quoted;
    }
    if add_stdin_redirect {
        format!("{quoted} < /dev/null")
    } else {
        quoted
    }
}

/// Defensive rewrite of Windows CMD-style null redirects.
///
/// The model occasionally emits `2>nul` even when the executing shell is
/// POSIX bash (Git Bash / WSL / native Linux). In POSIX shells `nul` is
/// just a filename — bash creates a literal file named `nul` which is a
/// reserved Windows device name and breaks `git add .` / `git clone`.
///
/// Matches: `>nul`, `> NUL`, `2>nul`, `&>nul`, `>>nul` (case-insensitive).
/// Does NOT match: `>null`, `>nullable`, `>nul.txt`, `cat nul.txt`.
pub fn rewrite_windows_null_redirect(command: &str) -> String {
    let mut out = String::with_capacity(command.len());
    let bytes = command.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Try to match `(\d?&?>+\s*)nul(?=[\s|&;)\n]|$)` case-insensitive.
        let prefix_start = i;
        let mut j = i;
        // Optional leading digit (\d?)
        if j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        // Optional &
        if j < bytes.len() && bytes[j] == b'&' {
            j += 1;
        }
        // Required >+
        let gt_start = j;
        while j < bytes.len() && bytes[j] == b'>' {
            j += 1;
        }
        if j == gt_start {
            // No `>` — emit one byte and continue.
            out.push(bytes[i] as char);
            i += 1;
            continue;
        }
        // Optional whitespace — TS regex `(\d?&?>+\s*)` captures it
        // *inside* group 1, so the rewrite preserves spacing
        // (`ls > NUL` → `ls > /dev/null`, not `ls >/dev/null`).
        while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
            j += 1;
        }
        // Required `nul` / `NUL` / `Nul` etc.
        if j + 2 < bytes.len()
            && bytes[j].eq_ignore_ascii_case(&b'n')
            && bytes[j + 1].eq_ignore_ascii_case(&b'u')
            && bytes[j + 2].eq_ignore_ascii_case(&b'l')
        {
            let after = j + 3;
            // Followed by whitespace, command separator, or EOF.
            let followed_ok = after == bytes.len()
                || matches!(
                    bytes[after],
                    b' ' | b'\t' | b'\n' | b'\r' | b'|' | b'&' | b';' | b')'
                );
            if followed_ok {
                // Emit the captured prefix (digit/amp + >+ + spaces)
                // up to the `n`, then /dev/null. Preserves spacing.
                out.push_str(&command[prefix_start..j]);
                out.push_str("/dev/null");
                i = after;
                continue;
            }
        }
        // No match — emit one byte and continue.
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

#[cfg(test)]
#[path = "shell_quoting.test.rs"]
mod tests;
