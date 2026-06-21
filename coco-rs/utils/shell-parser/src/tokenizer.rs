//! Shell command tokenizer with support for quotes and heredocs.

use crate::error::Result;

/// A span in the source string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// Start byte offset (inclusive).
    pub start: i32,
    /// End byte offset (exclusive).
    pub end: i32,
}

impl Span {
    /// Create a new span.
    pub fn new(start: i32, end: i32) -> Self {
        Self { start, end }
    }

    /// Returns the length of the span.
    pub fn len(&self) -> i32 {
        self.end - self.start
    }

    /// Returns true if the span is empty.
    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }
}

/// Token kinds in shell commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// A plain word (unquoted).
    Word,
    /// Single-quoted string ('...').
    SingleQuoted,
    /// Double-quoted string ("...").
    DoubleQuoted,
    /// ANSI-C quoting ($'...').
    AnsiCQuoted,
    /// Localized string ($"...").
    LocalizedString,
    /// Heredoc content.
    Heredoc,
    /// Operators like |, &&, ||, ;, &, etc.
    Operator,
    /// Command substitution $(...) or `...`.
    CommandSubstitution,
    /// Variable expansion $VAR or ${VAR}.
    VariableExpansion,
    /// Process substitution <(...) or >(...).
    ProcessSubstitution,
    /// Redirection operators >, >>, <, etc.
    Redirect,
    /// Comment starting with #.
    Comment,
    /// Whitespace.
    Whitespace,
    /// Unknown or unrecognized token.
    Unknown,
}

/// A token in a shell command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// The kind of token.
    pub kind: TokenKind,
    /// The raw text of the token.
    pub text: String,
    /// The span in the source string.
    pub span: Span,
}

impl Token {
    /// Create a new token.
    pub fn new(kind: TokenKind, text: String, span: Span) -> Self {
        Self { kind, text, span }
    }

    /// Returns the unquoted content for quoted tokens.
    pub fn unquoted_content(&self) -> &str {
        match self.kind {
            TokenKind::SingleQuoted => {
                // Strip surrounding quotes
                self.text
                    .strip_prefix('\'')
                    .and_then(|s| s.strip_suffix('\''))
                    .unwrap_or(&self.text)
            }
            TokenKind::DoubleQuoted => {
                // Strip surrounding quotes
                self.text
                    .strip_prefix('"')
                    .and_then(|s| s.strip_suffix('"'))
                    .unwrap_or(&self.text)
            }
            TokenKind::AnsiCQuoted => {
                // Strip $' prefix and ' suffix
                self.text
                    .strip_prefix("$'")
                    .and_then(|s| s.strip_suffix('\''))
                    .unwrap_or(&self.text)
            }
            TokenKind::LocalizedString => {
                // Strip $" prefix and " suffix
                self.text
                    .strip_prefix("$\"")
                    .and_then(|s| s.strip_suffix('"'))
                    .unwrap_or(&self.text)
            }
            _ => &self.text,
        }
    }
}

/// Shell tokenizer that handles quotes, operators, and heredocs.
#[derive(Debug, Default)]
pub struct Tokenizer {
    // Future: could hold configuration options
}

impl Tokenizer {
    /// Create a new tokenizer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Tokenize a shell command string.
    pub fn tokenize(&self, input: &str) -> Result<Vec<Token>> {
        let mut tokens = Vec::new();
        let mut pos = 0;
        let bytes = input.as_bytes();

        while pos < bytes.len() {
            let start_pos = pos;

            match bytes[pos] {
                // Whitespace
                b' ' | b'\t' | b'\n' | b'\r' => {
                    while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t' | b'\n' | b'\r') {
                        pos += 1;
                    }
                    tokens.push(Token::new(
                        TokenKind::Whitespace,
                        input[start_pos..pos].to_string(),
                        Span::new(start_pos as i32, pos as i32),
                    ));
                }

                // Comment
                b'#' => {
                    while pos < bytes.len() && bytes[pos] != b'\n' {
                        pos += 1;
                    }
                    tokens.push(Token::new(
                        TokenKind::Comment,
                        input[start_pos..pos].to_string(),
                        Span::new(start_pos as i32, pos as i32),
                    ));
                }

                // Single-quoted string
                b'\'' => {
                    pos += 1;
                    while pos < bytes.len() && bytes[pos] != b'\'' {
                        pos += 1;
                    }
                    if pos < bytes.len() {
                        pos += 1; // consume closing quote
                    }
                    tokens.push(Token::new(
                        TokenKind::SingleQuoted,
                        input[start_pos..pos].to_string(),
                        Span::new(start_pos as i32, pos as i32),
                    ));
                }

                // Double-quoted string
                b'"' => {
                    pos += 1;
                    while pos < bytes.len() {
                        if bytes[pos] == b'"' {
                            break;
                        }
                        if bytes[pos] == b'\\' && pos + 1 < bytes.len() {
                            pos += 2; // skip escaped char
                        } else {
                            pos += 1;
                        }
                    }
                    if pos < bytes.len() {
                        pos += 1; // consume closing quote
                    }
                    tokens.push(Token::new(
                        TokenKind::DoubleQuoted,
                        input[start_pos..pos].to_string(),
                        Span::new(start_pos as i32, pos as i32),
                    ));
                }

                // $ - could be variable, ANSI-C quote, localized string, or command substitution
                b'$' => {
                    if pos + 1 < bytes.len() {
                        match bytes[pos + 1] {
                            // ANSI-C quoting $'...'
                            b'\'' => {
                                pos += 2;
                                while pos < bytes.len() {
                                    if bytes[pos] == b'\'' {
                                        break;
                                    }
                                    if bytes[pos] == b'\\' && pos + 1 < bytes.len() {
                                        pos += 2;
                                    } else {
                                        pos += 1;
                                    }
                                }
                                if pos < bytes.len() {
                                    pos += 1;
                                }
                                tokens.push(Token::new(
                                    TokenKind::AnsiCQuoted,
                                    input[start_pos..pos].to_string(),
                                    Span::new(start_pos as i32, pos as i32),
                                ));
                            }

                            // Localized string $"..."
                            b'"' => {
                                pos += 2;
                                while pos < bytes.len() {
                                    if bytes[pos] == b'"' {
                                        break;
                                    }
                                    if bytes[pos] == b'\\' && pos + 1 < bytes.len() {
                                        pos += 2;
                                    } else {
                                        pos += 1;
                                    }
                                }
                                if pos < bytes.len() {
                                    pos += 1;
                                }
                                tokens.push(Token::new(
                                    TokenKind::LocalizedString,
                                    input[start_pos..pos].to_string(),
                                    Span::new(start_pos as i32, pos as i32),
                                ));
                            }

                            // Command substitution $(...)
                            b'(' => {
                                pos += 2;
                                let mut depth = 1;
                                while pos < bytes.len() && depth > 0 {
                                    match bytes[pos] {
                                        b'(' => depth += 1,
                                        b')' => depth -= 1,
                                        b'\\' if pos + 1 < bytes.len() => pos += 1,
                                        _ => {}
                                    }
                                    pos += 1;
                                }
                                tokens.push(Token::new(
                                    TokenKind::CommandSubstitution,
                                    input[start_pos..pos].to_string(),
                                    Span::new(start_pos as i32, pos as i32),
                                ));
                            }

                            // Variable expansion ${...} or $VAR
                            b'{' => {
                                pos += 2;
                                let mut depth = 1;
                                while pos < bytes.len() && depth > 0 {
                                    match bytes[pos] {
                                        b'{' => depth += 1,
                                        b'}' => depth -= 1,
                                        b'\\' if pos + 1 < bytes.len() => pos += 1,
                                        _ => {}
                                    }
                                    pos += 1;
                                }
                                tokens.push(Token::new(
                                    TokenKind::VariableExpansion,
                                    input[start_pos..pos].to_string(),
                                    Span::new(start_pos as i32, pos as i32),
                                ));
                            }

                            // Simple variable $VAR
                            c if c.is_ascii_alphabetic() || c == b'_' => {
                                pos += 1;
                                while pos < bytes.len()
                                    && (bytes[pos].is_ascii_alphanumeric() || bytes[pos] == b'_')
                                {
                                    pos += 1;
                                }
                                tokens.push(Token::new(
                                    TokenKind::VariableExpansion,
                                    input[start_pos..pos].to_string(),
                                    Span::new(start_pos as i32, pos as i32),
                                ));
                            }

                            // Special variables like $?, $!, $$, etc.
                            c if matches!(c, b'?' | b'!' | b'$' | b'#' | b'*' | b'@' | b'-')
                                || c.is_ascii_digit() =>
                            {
                                pos += 2;
                                tokens.push(Token::new(
                                    TokenKind::VariableExpansion,
                                    input[start_pos..pos].to_string(),
                                    Span::new(start_pos as i32, pos as i32),
                                ));
                            }

                            _ => {
                                // Just a $ followed by something else - treat as word
                                pos = self.scan_word(input, pos);
                                tokens.push(Token::new(
                                    TokenKind::Word,
                                    input[start_pos..pos].to_string(),
                                    Span::new(start_pos as i32, pos as i32),
                                ));
                            }
                        }
                    } else {
                        // Just a $ at end - treat as word
                        pos += 1;
                        tokens.push(Token::new(
                            TokenKind::Word,
                            input[start_pos..pos].to_string(),
                            Span::new(start_pos as i32, pos as i32),
                        ));
                    }
                }

                // Backtick command substitution
                b'`' => {
                    pos += 1;
                    while pos < bytes.len() && bytes[pos] != b'`' {
                        if bytes[pos] == b'\\' && pos + 1 < bytes.len() {
                            pos += 2;
                        } else {
                            pos += 1;
                        }
                    }
                    if pos < bytes.len() {
                        pos += 1;
                    }
                    tokens.push(Token::new(
                        TokenKind::CommandSubstitution,
                        input[start_pos..pos].to_string(),
                        Span::new(start_pos as i32, pos as i32),
                    ));
                }

                // Process substitution <(...) or >(...)
                b'<' | b'>' if pos + 1 < bytes.len() && bytes[pos + 1] == b'(' => {
                    pos += 2;
                    let mut depth = 1;
                    while pos < bytes.len() && depth > 0 {
                        match bytes[pos] {
                            b'(' => depth += 1,
                            b')' => depth -= 1,
                            _ => {}
                        }
                        pos += 1;
                    }
                    tokens.push(Token::new(
                        TokenKind::ProcessSubstitution,
                        input[start_pos..pos].to_string(),
                        Span::new(start_pos as i32, pos as i32),
                    ));
                }

                // Redirections
                b'>' => {
                    pos += 1;
                    // >>, >&, etc.
                    if pos < bytes.len() && matches!(bytes[pos], b'>' | b'&' | b'|') {
                        pos += 1;
                    }
                    tokens.push(Token::new(
                        TokenKind::Redirect,
                        input[start_pos..pos].to_string(),
                        Span::new(start_pos as i32, pos as i32),
                    ));
                }

                b'<' => {
                    pos += 1;
                    // <<, <<<, <&, etc.
                    if pos < bytes.len() {
                        match bytes[pos] {
                            b'<' => {
                                pos += 1;
                                // <<< here-string
                                if pos < bytes.len() && bytes[pos] == b'<' {
                                    pos += 1;
                                }
                            }
                            b'&' | b'>' => pos += 1,
                            _ => {}
                        }
                    }
                    // Check for heredoc
                    if &input[start_pos..pos] == "<<" {
                        // Skip optional '-' for <<-
                        if pos < bytes.len() && bytes[pos] == b'-' {
                            pos += 1;
                        }
                        // Skip whitespace to find delimiter
                        while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t') {
                            pos += 1;
                        }
                        // Find the delimiter
                        let delim_start = pos;
                        let (delimiter, _quoted) = self.scan_heredoc_delimiter(input, &mut pos);

                        if !delimiter.is_empty() {
                            // Find the heredoc body
                            if let Some(heredoc_end) =
                                self.find_heredoc_body(input, pos, &delimiter)
                            {
                                // Include everything up to and including the delimiter line
                                tokens.push(Token::new(
                                    TokenKind::Heredoc,
                                    input[start_pos..heredoc_end].to_string(),
                                    Span::new(start_pos as i32, heredoc_end as i32),
                                ));
                                pos = heredoc_end;
                            } else {
                                // No closing delimiter found
                                tokens.push(Token::new(
                                    TokenKind::Redirect,
                                    input[start_pos..delim_start].to_string(),
                                    Span::new(start_pos as i32, delim_start as i32),
                                ));
                                pos = delim_start;
                            }
                        }
                    } else {
                        tokens.push(Token::new(
                            TokenKind::Redirect,
                            input[start_pos..pos].to_string(),
                            Span::new(start_pos as i32, pos as i32),
                        ));
                    }
                }

                // Operators
                b'|' => {
                    pos += 1;
                    // || or |&
                    if pos < bytes.len() && matches!(bytes[pos], b'|' | b'&') {
                        pos += 1;
                    }
                    tokens.push(Token::new(
                        TokenKind::Operator,
                        input[start_pos..pos].to_string(),
                        Span::new(start_pos as i32, pos as i32),
                    ));
                }

                b'&' => {
                    pos += 1;
                    // && or &> or &>>
                    if pos < bytes.len() {
                        if bytes[pos] == b'&' {
                            pos += 1;
                        } else if bytes[pos] == b'>' {
                            pos += 1;
                            if pos < bytes.len() && bytes[pos] == b'>' {
                                pos += 1;
                            }
                            tokens.push(Token::new(
                                TokenKind::Redirect,
                                input[start_pos..pos].to_string(),
                                Span::new(start_pos as i32, pos as i32),
                            ));
                            continue;
                        }
                    }
                    tokens.push(Token::new(
                        TokenKind::Operator,
                        input[start_pos..pos].to_string(),
                        Span::new(start_pos as i32, pos as i32),
                    ));
                }

                b';' => {
                    pos += 1;
                    // ;; for case statements
                    if pos < bytes.len() && bytes[pos] == b';' {
                        pos += 1;
                    }
                    tokens.push(Token::new(
                        TokenKind::Operator,
                        input[start_pos..pos].to_string(),
                        Span::new(start_pos as i32, pos as i32),
                    ));
                }

                b'(' | b')' | b'{' | b'}' | b'[' | b']' => {
                    pos += 1;
                    tokens.push(Token::new(
                        TokenKind::Operator,
                        input[start_pos..pos].to_string(),
                        Span::new(start_pos as i32, pos as i32),
                    ));
                }

                b'!' => {
                    pos += 1;
                    tokens.push(Token::new(
                        TokenKind::Operator,
                        input[start_pos..pos].to_string(),
                        Span::new(start_pos as i32, pos as i32),
                    ));
                }

                // Number followed by redirection (e.g., 2>)
                c if c.is_ascii_digit() => {
                    let digit_start = pos;
                    while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                        pos += 1;
                    }
                    // Check if followed by redirection
                    if pos < bytes.len() && matches!(bytes[pos], b'>' | b'<') {
                        // This is a file descriptor redirection like 2>
                        pos += 1;
                        if pos < bytes.len() && matches!(bytes[pos], b'>' | b'<' | b'&') {
                            pos += 1;
                        }
                        tokens.push(Token::new(
                            TokenKind::Redirect,
                            input[digit_start..pos].to_string(),
                            Span::new(digit_start as i32, pos as i32),
                        ));
                    } else {
                        // Just a word starting with digits
                        pos = self.scan_word(input, digit_start);
                        tokens.push(Token::new(
                            TokenKind::Word,
                            input[digit_start..pos].to_string(),
                            Span::new(digit_start as i32, pos as i32),
                        ));
                    }
                }

                // Regular word
                _ => {
                    pos = self.scan_word(input, pos);
                    tokens.push(Token::new(
                        TokenKind::Word,
                        input[start_pos..pos].to_string(),
                        Span::new(start_pos as i32, pos as i32),
                    ));
                }
            }
        }

        Ok(tokens)
    }

    /// Scan a word starting at the given position.
    fn scan_word(&self, input: &str, mut pos: usize) -> usize {
        let bytes = input.as_bytes();

        while pos < bytes.len() {
            match bytes[pos] {
                // Word terminators
                b' ' | b'\t' | b'\n' | b'\r' | b'\'' | b'"' | b'`' | b'|' | b'&' | b';' | b'('
                | b')' | b'{' | b'}' | b'<' | b'>' | b'#' | b'$' => break,
                // Escaped character
                b'\\' if pos + 1 < bytes.len() => pos += 2,
                _ => pos += 1,
            }
        }

        pos
    }

    /// Scan a heredoc delimiter and return (delimiter, is_quoted).
    fn scan_heredoc_delimiter(&self, input: &str, pos: &mut usize) -> (String, bool) {
        let bytes = input.as_bytes();
        let start = *pos;
        let mut quoted = false;

        if *pos >= bytes.len() {
            return (String::new(), false);
        }

        match bytes[*pos] {
            b'\'' => {
                quoted = true;
                *pos += 1;
                let delim_start = *pos;
                while *pos < bytes.len() && bytes[*pos] != b'\'' {
                    *pos += 1;
                }
                let delimiter = input[delim_start..*pos].to_string();
                if *pos < bytes.len() {
                    *pos += 1; // consume closing quote
                }
                (delimiter, quoted)
            }
            b'"' => {
                quoted = true;
                *pos += 1;
                let delim_start = *pos;
                while *pos < bytes.len() && bytes[*pos] != b'"' {
                    *pos += 1;
                }
                let delimiter = input[delim_start..*pos].to_string();
                if *pos < bytes.len() {
                    *pos += 1; // consume closing quote
                }
                (delimiter, quoted)
            }
            _ => {
                // Unquoted delimiter
                while *pos < bytes.len() && !matches!(bytes[*pos], b' ' | b'\t' | b'\n' | b'\r') {
                    *pos += 1;
                }
                (input[start..*pos].to_string(), quoted)
            }
        }
    }

    /// Find the end of a heredoc body given the delimiter.
    fn find_heredoc_body(&self, input: &str, start: usize, delimiter: &str) -> Option<usize> {
        let bytes = input.as_bytes();
        let mut pos = start;

        // Skip to end of current line
        while pos < bytes.len() && bytes[pos] != b'\n' {
            pos += 1;
        }
        if pos < bytes.len() {
            pos += 1; // skip newline
        }

        // Look for the delimiter on its own line
        while pos < bytes.len() {
            let _line_start = pos;
            // Skip leading whitespace (for <<-)
            while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t') {
                pos += 1;
            }
            // Check if line matches delimiter
            let remaining = &input[pos..];
            if remaining.starts_with(delimiter) {
                let after_delim = pos + delimiter.len();
                // Ensure delimiter is at end of line or followed by newline
                if after_delim >= bytes.len()
                    || bytes[after_delim] == b'\n'
                    || (bytes[after_delim] == b'\r'
                        && after_delim + 1 < bytes.len()
                        && bytes[after_delim + 1] == b'\n')
                {
                    // Include the delimiter line
                    let mut end = after_delim;
                    if end < bytes.len() && bytes[end] == b'\r' {
                        end += 1;
                    }
                    if end < bytes.len() && bytes[end] == b'\n' {
                        end += 1;
                    }
                    return Some(end);
                }
            }
            // Move to next line
            while pos < bytes.len() && bytes[pos] != b'\n' {
                pos += 1;
            }
            if pos < bytes.len() {
                pos += 1;
            }
        }

        None
    }
}

#[cfg(test)]
#[path = "tokenizer.test.rs"]
mod tests;
