//! Shell tokenizer that splits command strings into tokens respecting quotes,
//! escapes, heredocs, and variable expansions.

/// Token types produced by the shell tokenizer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// A plain word (identifier, path, flag, etc.).
    Word,
    /// A numeric literal.
    Number,
    /// An operator (|, &&, ||, ;, >, >>, etc.).
    Operator,
    /// A newline character.
    Newline,
    /// A shell comment (#...).
    Comment,
    /// A single-quoted string ('...').
    SingleQuoted,
    /// A double-quoted string ("...").
    DoubleQuoted,
    /// An ANSI-C string ($'...').
    AnsiC,
    /// A bare dollar sign ($).
    Dollar,
    /// Command substitution start: $(.
    DollarParen,
    /// Parameter expansion start: ${.
    DollarBrace,
    /// Arithmetic expansion start: $((.
    DollarDoubleParen,
    /// Backtick for legacy command substitution.
    Backtick,
    /// Process substitution: <(.
    ProcessSubIn,
    /// Process substitution: >(.
    ProcessSubOut,
    /// End of input.
    Eof,
}

/// A single token from the shell tokenizer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub value: String,
    /// Byte offset of token start.
    pub start: usize,
    /// Byte offset of token end.
    pub end: usize,
}

/// Context for tokenization — affects how certain characters are treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LexContext {
    /// Command position: `[`, `[[`, `{` are operators.
    Command,
    /// Argument position: `[` is a word character (glob/subscript).
    Argument,
}

/// Lexer state.
struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src: src.as_bytes(),
            pos: 0,
        }
    }

    fn peek(&self) -> u8 {
        if self.pos < self.src.len() {
            self.src[self.pos]
        } else {
            0
        }
    }

    fn peek_at(&self, offset: usize) -> u8 {
        let idx = self.pos + offset;
        if idx < self.src.len() {
            self.src[idx]
        } else {
            0
        }
    }

    fn advance(&mut self) {
        if self.pos < self.src.len() {
            self.pos += 1;
        }
    }

    fn at_end(&self) -> bool {
        self.pos >= self.src.len()
    }

    fn skip_blanks(&mut self) {
        while self.pos < self.src.len() {
            match self.src[self.pos] {
                b' ' | b'\t' | b'\r' => self.advance(),
                b'\\' if self.pos + 1 < self.src.len() => {
                    let next = self.src[self.pos + 1];
                    if next == b'\n' {
                        // Line continuation
                        self.advance();
                        self.advance();
                    } else if next == b' ' || next == b'\t' {
                        // Escaped whitespace
                        self.advance();
                        self.advance();
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }
    }

    fn slice(&self, start: usize, end: usize) -> &'a str {
        // Safety: we always operate on valid UTF-8 (from &str input)
        std::str::from_utf8(&self.src[start..end]).unwrap_or("")
    }
}

fn is_word_char(c: u8) -> bool {
    matches!(c,
        b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9'
        | b'_' | b'/' | b'.' | b'-' | b'+' | b':' | b'@'
        | b'%' | b',' | b'~' | b'^' | b'?' | b'*' | b'!'
        | b'=' | b'[' | b']'
    )
}

fn is_word_start(c: u8) -> bool {
    is_word_char(c) || c == b'\\'
}

fn is_digit(c: u8) -> bool {
    c.is_ascii_digit()
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

fn is_ident_char(c: u8) -> bool {
    is_ident_start(c) || c.is_ascii_digit()
}

/// Tokenize a shell command string into a list of tokens.
pub fn tokenize(input: &str) -> Vec<Token> {
    let mut lexer = Lexer::new(input);
    let mut tokens = Vec::new();

    loop {
        let token = next_token(&mut lexer, LexContext::Argument);
        let is_eof = token.kind == TokenKind::Eof;
        tokens.push(token);
        if is_eof {
            break;
        }
    }

    tokens
}

/// Tokenize with context sensitivity (command vs argument position).
pub fn tokenize_with_context(input: &str) -> Vec<Token> {
    let mut lexer = Lexer::new(input);
    let mut tokens = Vec::new();
    let mut ctx = LexContext::Command;

    loop {
        let token = next_token(&mut lexer, ctx);
        let is_eof = token.kind == TokenKind::Eof;

        // After a word/number/quoted-string in command position, switch to argument
        ctx = match token.kind {
            TokenKind::Operator | TokenKind::Newline => LexContext::Command,
            TokenKind::Eof => LexContext::Command,
            _ => LexContext::Argument,
        };

        tokens.push(token);
        if is_eof {
            break;
        }
    }

    tokens
}

fn next_token(lex: &mut Lexer<'_>, ctx: LexContext) -> Token {
    lex.skip_blanks();
    let start = lex.pos;

    if lex.at_end() {
        return Token {
            kind: TokenKind::Eof,
            value: String::new(),
            start,
            end: start,
        };
    }

    let c = lex.peek();
    let c1 = lex.peek_at(1);
    let c2 = lex.peek_at(2);

    // Newline
    if c == b'\n' {
        lex.advance();
        return Token {
            kind: TokenKind::Newline,
            value: "\n".to_string(),
            start,
            end: lex.pos,
        };
    }

    // Comment
    if c == b'#' {
        let s = lex.pos;
        while !lex.at_end() && lex.peek() != b'\n' {
            lex.advance();
        }
        return Token {
            kind: TokenKind::Comment,
            value: lex.slice(s, lex.pos).to_string(),
            start,
            end: lex.pos,
        };
    }

    // Multi-char operators (longest match first)
    if let Some(tok) = try_multi_char_op(lex, start) {
        return tok;
    }

    // Single-char operators
    if matches!(c, b'|' | b'&' | b';' | b'>' | b'<') {
        lex.advance();
        return Token {
            kind: TokenKind::Operator,
            value: String::from(c as char),
            start,
            end: lex.pos,
        };
    }
    if matches!(c, b'(' | b')') {
        lex.advance();
        return Token {
            kind: TokenKind::Operator,
            value: String::from(c as char),
            start,
            end: lex.pos,
        };
    }

    // Context-sensitive operators in command position
    if ctx == LexContext::Command {
        if c == b'[' && c1 == b'[' {
            lex.advance();
            lex.advance();
            return Token {
                kind: TokenKind::Operator,
                value: "[[".to_string(),
                start,
                end: lex.pos,
            };
        }
        if c == b'[' {
            lex.advance();
            return Token {
                kind: TokenKind::Operator,
                value: "[".to_string(),
                start,
                end: lex.pos,
            };
        }
        if c == b'{' && matches!(c1, b' ' | b'\t' | b'\n') {
            lex.advance();
            return Token {
                kind: TokenKind::Operator,
                value: "{".to_string(),
                start,
                end: lex.pos,
            };
        }
        if c == b'}' {
            lex.advance();
            return Token {
                kind: TokenKind::Operator,
                value: "}".to_string(),
                start,
                end: lex.pos,
            };
        }
        if c == b'!' && matches!(c1, b' ' | b'\t') {
            lex.advance();
            return Token {
                kind: TokenKind::Operator,
                value: "!".to_string(),
                start,
                end: lex.pos,
            };
        }
    }

    // Double-quoted string
    if c == b'"' {
        return scan_double_quoted(lex, start);
    }

    // Single-quoted string
    if c == b'\'' {
        return scan_single_quoted(lex, start);
    }

    // Dollar-prefixed tokens
    if c == b'$' {
        return scan_dollar(lex, start, c1, c2);
    }

    // Backtick
    if c == b'`' {
        lex.advance();
        return Token {
            kind: TokenKind::Backtick,
            value: "`".to_string(),
            start,
            end: lex.pos,
        };
    }

    // File descriptor before redirect: digit+ immediately followed by > or <
    if is_digit(c) {
        let mut j = lex.pos;
        while j < lex.src.len() && is_digit(lex.src[j]) {
            j += 1;
        }
        if j < lex.src.len() && (lex.src[j] == b'>' || lex.src[j] == b'<') {
            let s = lex.pos;
            while lex.pos < j {
                lex.advance();
            }
            return Token {
                kind: TokenKind::Word,
                value: lex.slice(s, lex.pos).to_string(),
                start,
                end: lex.pos,
            };
        }
    }

    // Word / number
    if is_word_start(c) || c == b'{' || c == b'}' {
        return scan_word(lex, start);
    }

    // Unknown char — consume as single-char word
    lex.advance();
    Token {
        kind: TokenKind::Word,
        value: String::from(c as char),
        start,
        end: lex.pos,
    }
}

fn try_multi_char_op(lex: &mut Lexer<'_>, start: usize) -> Option<Token> {
    let c = lex.peek();
    let c1 = lex.peek_at(1);
    let c2 = lex.peek_at(2);

    let (value, len): (&str, usize) = match (c, c1, c2) {
        // Three-char operators
        (b'<', b'<', b'<') => ("<<<", 3),
        (b'<', b'<', b'-') => ("<<-", 3),
        (b'>', b'>', _) if c2 != b'>' => (">>", 2),
        (b'>', b'&', b'-') => (">&-", 3),
        (b'<', b'&', b'-') => ("<&-", 3),
        (b'&', b'>', b'>') => ("&>>", 3),
        (b';', b';', b'&') => (";;&", 3),
        (b'(', b'(', _) => ("((", 2),
        (b')', b')', _) => ("))", 2),
        // Two-char operators
        (b'&', b'&', _) => ("&&", 2),
        (b'|', b'|', _) => ("||", 2),
        (b'|', b'&', _) => ("|&", 2),
        (b';', b';', _) => (";;", 2),
        (b';', b'&', _) => (";&", 2),
        (b'>', b'&', _) => (">&", 2),
        (b'>', b'|', _) => (">|", 2),
        (b'&', b'>', _) => ("&>", 2),
        (b'<', b'<', _) => ("<<", 2),
        (b'<', b'&', _) => ("<&", 2),
        (b'<', b'(', _) => {
            lex.advance();
            lex.advance();
            return Some(Token {
                kind: TokenKind::ProcessSubIn,
                value: "<(".to_string(),
                start,
                end: lex.pos,
            });
        }
        (b'>', b'(', _) => {
            lex.advance();
            lex.advance();
            return Some(Token {
                kind: TokenKind::ProcessSubOut,
                value: ">(".to_string(),
                start,
                end: lex.pos,
            });
        }
        _ => return None,
    };

    for _ in 0..len {
        lex.advance();
    }
    Some(Token {
        kind: TokenKind::Operator,
        value: value.to_string(),
        start,
        end: lex.pos,
    })
}

fn scan_double_quoted(lex: &mut Lexer<'_>, start: usize) -> Token {
    let s = lex.pos;
    lex.advance(); // skip opening "
    let mut depth = 1_i32;
    while !lex.at_end() && depth > 0 {
        match lex.peek() {
            b'"' => {
                depth -= 1;
                lex.advance();
            }
            b'\\' if lex.pos + 1 < lex.src.len() => {
                lex.advance();
                lex.advance();
            }
            _ => lex.advance(),
        }
    }
    Token {
        kind: TokenKind::DoubleQuoted,
        value: lex.slice(s, lex.pos).to_string(),
        start,
        end: lex.pos,
    }
}

fn scan_single_quoted(lex: &mut Lexer<'_>, start: usize) -> Token {
    let s = lex.pos;
    lex.advance(); // skip opening '
    while !lex.at_end() && lex.peek() != b'\'' {
        lex.advance();
    }
    if !lex.at_end() {
        lex.advance(); // skip closing '
    }
    Token {
        kind: TokenKind::SingleQuoted,
        value: lex.slice(s, lex.pos).to_string(),
        start,
        end: lex.pos,
    }
}

fn scan_dollar(lex: &mut Lexer<'_>, start: usize, c1: u8, c2: u8) -> Token {
    if c1 == b'(' && c2 == b'(' {
        lex.advance();
        lex.advance();
        lex.advance();
        return Token {
            kind: TokenKind::DollarDoubleParen,
            value: "$((".to_string(),
            start,
            end: lex.pos,
        };
    }
    if c1 == b'(' {
        lex.advance();
        lex.advance();
        return Token {
            kind: TokenKind::DollarParen,
            value: "$(".to_string(),
            start,
            end: lex.pos,
        };
    }
    if c1 == b'{' {
        lex.advance();
        lex.advance();
        return Token {
            kind: TokenKind::DollarBrace,
            value: "${".to_string(),
            start,
            end: lex.pos,
        };
    }
    if c1 == b'\'' {
        // ANSI-C string $'...'
        let s = lex.pos;
        lex.advance(); // $
        lex.advance(); // '
        while !lex.at_end() && lex.peek() != b'\'' {
            if lex.peek() == b'\\' && lex.pos + 1 < lex.src.len() {
                lex.advance();
            }
            lex.advance();
        }
        if !lex.at_end() {
            lex.advance(); // closing '
        }
        return Token {
            kind: TokenKind::AnsiC,
            value: lex.slice(s, lex.pos).to_string(),
            start,
            end: lex.pos,
        };
    }
    // Bare $
    lex.advance();
    Token {
        kind: TokenKind::Dollar,
        value: "$".to_string(),
        start,
        end: lex.pos,
    }
}

fn scan_word(lex: &mut Lexer<'_>, start: usize) -> Token {
    let s = lex.pos;
    while !lex.at_end() {
        let ch = lex.peek();
        if ch == b'\\' {
            if lex.pos + 1 >= lex.src.len() {
                break; // trailing backslash
            }
            if lex.src[lex.pos + 1] == b'\n' {
                // line continuation mid-word
                lex.advance();
                lex.advance();
                continue;
            }
            // escape next char
            lex.advance();
            lex.advance();
            continue;
        }
        if !is_word_char(ch) && ch != b'{' && ch != b'}' {
            break;
        }
        lex.advance();
    }
    if lex.pos > s {
        let v = lex.slice(s, lex.pos);
        let kind = if is_number_literal(v) {
            TokenKind::Number
        } else {
            TokenKind::Word
        };
        Token {
            kind,
            value: v.to_string(),
            start,
            end: lex.pos,
        }
    } else {
        // empty word — consume one char
        lex.advance();
        Token {
            kind: TokenKind::Word,
            value: lex.slice(s, lex.pos).to_string(),
            start,
            end: lex.pos,
        }
    }
}

fn is_number_literal(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let start = if bytes[0] == b'-' { 1 } else { 0 };
    if start >= bytes.len() {
        return false;
    }
    bytes[start..].iter().all(u8::is_ascii_digit)
}

// ── Variable expansion detection ──

/// Detected variable expansion in a shell string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expansion {
    /// Simple variable: $VAR
    SimpleVar(String),
    /// Braced variable: ${VAR}, ${VAR:-default}, etc.
    BracedVar(String),
    /// Command substitution: $(cmd)
    CommandSub(String),
    /// Backtick command substitution: `cmd`
    BacktickSub(String),
    /// Arithmetic expansion: $((expr))
    ArithmeticExp(String),
}

/// Detect all variable expansions and command substitutions in a string.
pub fn detect_expansions(input: &str) -> Vec<Expansion> {
    let mut expansions = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                i += 2; // skip escaped char
            }
            b'\'' => {
                // Single-quoted strings have no expansions
                i += 1;
                while i < bytes.len() && bytes[i] != b'\'' {
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1;
                }
            }
            b'$' => {
                if i + 1 < bytes.len() {
                    match bytes[i + 1] {
                        b'(' if i + 2 < bytes.len() && bytes[i + 2] == b'(' => {
                            // Arithmetic: $((expr))
                            let start = i + 3;
                            let mut depth = 1_i32;
                            let mut j = start;
                            while j + 1 < bytes.len() && depth > 0 {
                                if bytes[j] == b'(' && bytes[j + 1] == b'(' {
                                    depth += 1;
                                    j += 2;
                                } else if bytes[j] == b')' && bytes[j + 1] == b')' {
                                    depth -= 1;
                                    if depth > 0 {
                                        j += 2;
                                    }
                                } else {
                                    j += 1;
                                }
                            }
                            let content = std::str::from_utf8(&bytes[start..j]).unwrap_or("");
                            expansions.push(Expansion::ArithmeticExp(content.to_string()));
                            i = j + 2;
                        }
                        b'(' => {
                            // Command substitution: $(cmd)
                            let start = i + 2;
                            let mut depth = 1_i32;
                            let mut j = start;
                            while j < bytes.len() && depth > 0 {
                                match bytes[j] {
                                    b'(' => depth += 1,
                                    b')' => depth -= 1,
                                    _ => {}
                                }
                                if depth > 0 {
                                    j += 1;
                                }
                            }
                            let content = std::str::from_utf8(&bytes[start..j]).unwrap_or("");
                            expansions.push(Expansion::CommandSub(content.to_string()));
                            i = j + 1;
                        }
                        b'{' => {
                            // Braced variable: ${VAR...}
                            let start = i + 2;
                            let mut depth = 1_i32;
                            let mut j = start;
                            while j < bytes.len() && depth > 0 {
                                match bytes[j] {
                                    b'{' => depth += 1,
                                    b'}' => depth -= 1,
                                    _ => {}
                                }
                                if depth > 0 {
                                    j += 1;
                                }
                            }
                            let content = std::str::from_utf8(&bytes[start..j]).unwrap_or("");
                            // Extract just the variable name (before any operator)
                            let var_name: String = content
                                .chars()
                                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                                .collect();
                            expansions.push(Expansion::BracedVar(var_name));
                            i = j + 1;
                        }
                        c if c.is_ascii_alphabetic() || c == b'_' => {
                            // Simple variable: $VAR
                            let start = i + 1;
                            let mut j = start;
                            while j < bytes.len() && is_ident_char(bytes[j]) {
                                j += 1;
                            }
                            let name = std::str::from_utf8(&bytes[start..j]).unwrap_or("");
                            expansions.push(Expansion::SimpleVar(name.to_string()));
                            i = j;
                        }
                        _ => {
                            // Special variables: $?, $$, $!, etc.
                            i += 1;
                        }
                    }
                } else {
                    i += 1;
                }
            }
            b'`' => {
                // Backtick command substitution
                i += 1;
                let start = i;
                while i < bytes.len() && bytes[i] != b'`' {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                let content = std::str::from_utf8(&bytes[start..i]).unwrap_or("");
                expansions.push(Expansion::BacktickSub(content.to_string()));
                if i < bytes.len() {
                    i += 1;
                }
            }
            _ => {
                i += 1;
            }
        }
    }

    expansions
}

/// Check if a string contains any variable expansions or command substitutions.
pub fn has_expansions(input: &str) -> bool {
    let bytes = input.as_bytes();
    let mut i = 0;
    let mut in_single = false;

    while i < bytes.len() {
        match bytes[i] {
            b'\\' if !in_single => {
                i += 2;
            }
            b'\'' => {
                in_single = !in_single;
                i += 1;
            }
            b'$' if !in_single => return true,
            b'`' if !in_single => return true,
            _ => {
                i += 1;
            }
        }
    }

    false
}

/// Check if a string contains a here-string operator (<<<).
pub fn has_here_string(input: &str) -> bool {
    let bytes = input.as_bytes();
    let mut i = 0;
    let mut in_single = false;
    let mut in_double = false;

    while i + 2 < bytes.len() {
        match bytes[i] {
            b'\'' if !in_double => {
                in_single = !in_single;
            }
            b'"' if !in_single => {
                in_double = !in_double;
            }
            b'<' if !in_single && !in_double => {
                if bytes[i + 1] == b'<' && bytes[i + 2] == b'<' {
                    // Make sure it's not part of a longer sequence
                    if i + 3 >= bytes.len() || bytes[i + 3] != b'<' {
                        return true;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }

    false
}

#[cfg(test)]
#[path = "tokenizer.test.rs"]
mod tests;
