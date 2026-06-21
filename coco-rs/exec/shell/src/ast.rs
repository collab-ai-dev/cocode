//! Bash AST types and parser with proper tokenization.
//!
//! Represents a simplified bash command AST for security analysis.
//! Uses the tokenizer for proper quote/escape handling.

use crate::tokenizer::Token;
use crate::tokenizer::TokenKind;
use crate::tokenizer::tokenize;
use serde::Deserialize;
use serde::Serialize;

/// A parsed bash command (simplified AST).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BashNode {
    /// A simple command: name + arguments.
    SimpleCommand(SimpleCommand),
    /// A pipeline: cmd1 | cmd2 | cmd3.
    Pipeline(Vec<BashNode>),
    /// A list: cmd1 && cmd2, cmd1 || cmd2, cmd1 ; cmd2.
    List {
        left: Box<BashNode>,
        operator: ListOperator,
        right: Box<BashNode>,
    },
    /// A subshell: ( commands ).
    Subshell(Box<BashNode>),
    /// Command substitution: $( commands ) or `commands`.
    CommandSubstitution(Box<BashNode>),
    /// A compound command (if/for/while/case).
    Compound { keyword: String, body: String },
    /// An assignment: VAR=value.
    Assignment { name: String, value: String },
    /// Unparseable command (fallback).
    Raw(String),
}

/// A simple command with its parts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimpleCommand {
    /// Environment variable assignments before the command.
    pub assignments: Vec<Assignment>,
    /// The command name (first word after assignments).
    pub command: String,
    /// Command arguments.
    pub args: Vec<String>,
    /// Redirections.
    pub redirects: Vec<Redirect>,
}

/// An environment variable assignment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assignment {
    pub name: String,
    pub value: String,
}

/// A shell redirection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Redirect {
    /// The file descriptor (0=stdin, 1=stdout, 2=stderr).
    pub fd: Option<i32>,
    /// The operator (>, >>, <, 2>, etc.).
    pub operator: RedirectOperator,
    /// The target (file path or fd number).
    pub target: String,
}

/// Redirect operator type.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RedirectOperator {
    /// > (write/truncate)
    Write,
    /// >> (append)
    Append,
    /// < (input)
    Input,
    /// 2> (stderr)
    Stderr,
    /// 2>> (stderr append)
    StderrAppend,
    /// &> or >& (both stdout+stderr)
    Both,
    /// <<< (here-string)
    HereString,
    /// << (heredoc)
    Heredoc,
    /// <<- (heredoc with tab stripping)
    HeredocStrip,
    /// >| (clobber)
    Clobber,
}

/// List operator.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ListOperator {
    /// && (and)
    And,
    /// || (or)
    Or,
    /// ; (sequential)
    Semicolon,
    /// & (background)
    Background,
}

/// Parse a command string into a simplified AST.
///
/// Uses the tokenizer for proper quote/escape handling. Falls back to Raw
/// for complex constructs.
pub fn parse_command(command: &str) -> BashNode {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return BashNode::Raw(String::new());
    }

    let tokens = tokenize(trimmed);
    parse_from_tokens(&tokens, trimmed)
}

/// Parse a command from pre-tokenized input.
fn parse_from_tokens(tokens: &[Token], original: &str) -> BashNode {
    // Filter out EOF, comments, newlines for structural analysis
    let meaningful: Vec<&Token> = tokens
        .iter()
        .filter(|t| {
            !matches!(
                t.kind,
                TokenKind::Eof | TokenKind::Comment | TokenKind::Newline
            )
        })
        .collect();

    if meaningful.is_empty() {
        return BashNode::Raw(String::new());
    }

    // Check for compound commands (if/for/while/case/until) BEFORE
    // splitting on operators — compound commands contain ; internally.
    if let Some(first) = meaningful.first() {
        let kw = first.value.as_str();
        if matches!(kw, "if" | "for" | "while" | "until" | "case" | "select") {
            return BashNode::Compound {
                keyword: kw.to_string(),
                body: original.to_string(),
            };
        }
    }

    // Try to split on list operators (&&, ||, ;, &)
    if let Some(node) = try_split_list(&meaningful, original) {
        return node;
    }

    // Try to split on pipe
    if let Some(node) = try_split_pipeline(&meaningful, original) {
        return node;
    }

    // Parse as simple command from tokens
    parse_simple_from_tokens(&meaningful, original)
}

/// Try to split on list operators, respecting parenthesis depth.
fn try_split_list(tokens: &[&Token], original: &str) -> Option<BashNode> {
    let mut depth = 0_i32;

    for (i, tok) in tokens.iter().enumerate() {
        match tok.kind {
            TokenKind::Operator if tok.value == "(" || tok.value == "((" => depth += 1,
            TokenKind::Operator if tok.value == ")" || tok.value == "))" => {
                depth = (depth - 1).max(0);
            }
            TokenKind::Operator if depth == 0 => {
                let operator = match tok.value.as_str() {
                    "&&" => Some(ListOperator::And),
                    "||" => Some(ListOperator::Or),
                    ";" => Some(ListOperator::Semicolon),
                    "&" => Some(ListOperator::Background),
                    _ => None,
                };

                if let Some(op) = operator {
                    let left_tokens = &tokens[..i];
                    let right_tokens = &tokens[i + 1..];

                    if left_tokens.is_empty() || right_tokens.is_empty() {
                        // Handle trailing separator
                        if right_tokens.is_empty() && !left_tokens.is_empty() {
                            let left_text = tokens_to_text(left_tokens, original);
                            return Some(BashNode::List {
                                left: Box::new(parse_command(&left_text)),
                                operator: op,
                                right: Box::new(BashNode::Raw(String::new())),
                            });
                        }
                        continue;
                    }

                    let left_text = tokens_to_text(left_tokens, original);
                    let right_text = tokens_to_text(right_tokens, original);

                    return Some(BashNode::List {
                        left: Box::new(parse_command(&left_text)),
                        operator: op,
                        right: Box::new(parse_command(&right_text)),
                    });
                }
            }
            _ => {}
        }
    }
    None
}

/// Try to split on pipe operators.
fn try_split_pipeline(tokens: &[&Token], original: &str) -> Option<BashNode> {
    let mut depth = 0_i32;
    let mut segments: Vec<Vec<&Token>> = Vec::new();
    let mut current: Vec<&Token> = Vec::new();

    for tok in tokens {
        match tok.kind {
            TokenKind::Operator if tok.value == "(" || tok.value == "((" => {
                depth += 1;
                current.push(tok);
            }
            TokenKind::Operator if tok.value == ")" || tok.value == "))" => {
                depth = (depth - 1).max(0);
                current.push(tok);
            }
            TokenKind::Operator if depth == 0 && (tok.value == "|" || tok.value == "|&") => {
                if !current.is_empty() {
                    segments.push(std::mem::take(&mut current));
                }
            }
            _ => {
                current.push(tok);
            }
        }
    }

    if !current.is_empty() {
        segments.push(current);
    }

    if segments.len() <= 1 {
        return None;
    }

    let nodes: Vec<BashNode> = segments
        .iter()
        .map(|seg| {
            let text = tokens_to_text(seg, original);
            parse_simple_from_tokens(seg, &text)
        })
        .collect();

    Some(BashNode::Pipeline(nodes))
}

/// Reconstruct text from a slice of tokens using original source spans.
fn tokens_to_text(tokens: &[&Token], original: &str) -> String {
    if tokens.is_empty() {
        return String::new();
    }
    let start = tokens[0].start;
    let end = tokens.last().map_or(start, |t| t.end);
    if end <= original.len() {
        original[start..end].to_string()
    } else {
        // Fallback: join token values
        tokens
            .iter()
            .map(|t| t.value.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Parse a simple command from tokens.
fn parse_simple_from_tokens(tokens: &[&Token], original: &str) -> BashNode {
    if tokens.is_empty() {
        return BashNode::Raw(String::new());
    }

    // Handle subshell: ( ... )
    if tokens.len() >= 2
        && tokens[0].kind == TokenKind::Operator
        && tokens[0].value == "("
        && tokens
            .last()
            .is_some_and(|t| t.kind == TokenKind::Operator && t.value == ")")
    {
        let inner = tokens_to_text(&tokens[1..tokens.len() - 1], original);
        return BashNode::Subshell(Box::new(parse_command(&inner)));
    }

    // Handle compound commands (if, for, while, case, until, select, function)
    if let Some(first_word) = tokens.first()
        && first_word.kind == TokenKind::Word
    {
        match first_word.value.as_str() {
            "if" | "for" | "while" | "until" | "case" | "select" | "function" => {
                let body = tokens_to_text(tokens, original);
                return BashNode::Compound {
                    keyword: first_word.value.clone(),
                    body,
                };
            }
            _ => {}
        }
    }

    let mut assignments = Vec::new();
    let mut cmd_start = 0;

    // Parse leading assignments (WORD tokens containing = with valid name before =)
    for (i, tok) in tokens.iter().enumerate() {
        if tok.kind == TokenKind::Word
            && let Some(eq_pos) = tok.value.find('=')
        {
            let name = &tok.value[..eq_pos];
            if !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && name.chars().next().is_some_and(|c| !c.is_ascii_digit())
            {
                assignments.push(Assignment {
                    name: name.to_string(),
                    value: tok.value[eq_pos + 1..].to_string(),
                });
                cmd_start = i + 1;
                continue;
            }
        }
        break;
    }

    let remaining = &tokens[cmd_start..];
    if remaining.is_empty() {
        return BashNode::SimpleCommand(SimpleCommand {
            assignments,
            command: String::new(),
            args: Vec::new(),
            redirects: Vec::new(),
        });
    }

    // First non-assignment word is the command name
    let command_name = resolve_token_value(&remaining[0].value, remaining[0].kind);
    let mut args = Vec::new();
    let mut redirects = Vec::new();
    let mut i = 1;

    while i < remaining.len() {
        let tok = remaining[i];

        // Check for redirect operators
        if tok.kind == TokenKind::Operator
            && let Some(redir) = try_parse_redirect(tok, remaining.get(i + 1).copied())
        {
            if redir.target.is_empty() {
                // Target is the next token
                if let Some(next) = remaining.get(i + 1) {
                    redirects.push(Redirect {
                        fd: redir.fd,
                        operator: redir.operator,
                        target: resolve_token_value(&next.value, next.kind),
                    });
                    i += 2;
                } else {
                    redirects.push(redir);
                    i += 1;
                }
            } else {
                redirects.push(redir);
                i += 1;
            }
            continue;
        }

        // Check for fd+redirect: a WORD that is just digits followed by a redirect operator
        if tok.kind == TokenKind::Word
            && tok.value.chars().all(|c| c.is_ascii_digit())
            && i + 1 < remaining.len()
        {
            let next = remaining[i + 1];
            if next.kind == TokenKind::Operator
                && let Some(mut redir) = try_parse_redirect(next, remaining.get(i + 2).copied())
            {
                redir.fd = tok.value.parse::<i32>().ok();
                if redir.target.is_empty() {
                    if let Some(target_tok) = remaining.get(i + 2) {
                        redirects.push(Redirect {
                            fd: redir.fd,
                            operator: redir.operator,
                            target: resolve_token_value(&target_tok.value, target_tok.kind),
                        });
                        i += 3;
                    } else {
                        redirects.push(redir);
                        i += 2;
                    }
                } else {
                    redirects.push(redir);
                    i += 2;
                }
                continue;
            }
        }

        // Regular argument
        args.push(resolve_token_value(&tok.value, tok.kind));
        i += 1;
    }

    BashNode::SimpleCommand(SimpleCommand {
        assignments,
        command: command_name,
        args,
        redirects,
    })
}

/// Try to parse a token as a redirect operator.
fn try_parse_redirect(tok: &Token, _next: Option<&Token>) -> Option<Redirect> {
    if tok.kind != TokenKind::Operator {
        return None;
    }
    let op = match tok.value.as_str() {
        ">" => RedirectOperator::Write,
        ">>" => RedirectOperator::Append,
        "<" => RedirectOperator::Input,
        "&>" | ">&" => RedirectOperator::Both,
        "&>>" => RedirectOperator::Both, // append both
        ">|" => RedirectOperator::Clobber,
        "<<<" => RedirectOperator::HereString,
        "<<" => RedirectOperator::Heredoc,
        "<<-" => RedirectOperator::HeredocStrip,
        _ => return None,
    };

    Some(Redirect {
        fd: None,
        operator: op,
        target: String::new(),
    })
}

/// Resolve a token value by stripping quotes.
fn resolve_token_value(value: &str, kind: TokenKind) -> String {
    match kind {
        TokenKind::SingleQuoted => {
            // Strip surrounding single quotes
            value
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
                .unwrap_or(value)
                .to_string()
        }
        TokenKind::DoubleQuoted => {
            // Strip surrounding double quotes and unescape
            let inner = value
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .unwrap_or(value);
            unescape_double_quoted(inner)
        }
        TokenKind::AnsiC => {
            // Strip $' prefix and ' suffix
            let inner = value
                .strip_prefix("$'")
                .and_then(|s| s.strip_suffix('\''))
                .unwrap_or(value);
            unescape_ansi_c(inner)
        }
        _ => {
            // For words, unescape backslash sequences
            unescape_word(value)
        }
    }
}

/// Unescape a double-quoted string.
fn unescape_double_quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'$' | b'`' | b'"' | b'\\' | b'\n' => {
                    if bytes[i + 1] != b'\n' {
                        out.push(bytes[i + 1] as char);
                    }
                    i += 2;
                }
                _ => {
                    out.push('\\');
                    i += 1;
                }
            }
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

/// Unescape an ANSI-C string ($'...').
fn unescape_ansi_c(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'n' => {
                    out.push('\n');
                    i += 2;
                }
                b't' => {
                    out.push('\t');
                    i += 2;
                }
                b'r' => {
                    out.push('\r');
                    i += 2;
                }
                b'a' => {
                    out.push('\x07');
                    i += 2;
                }
                b'b' => {
                    out.push('\x08');
                    i += 2;
                }
                b'e' | b'E' => {
                    out.push('\x1b');
                    i += 2;
                }
                b'f' => {
                    out.push('\x0c');
                    i += 2;
                }
                b'v' => {
                    out.push('\x0b');
                    i += 2;
                }
                b'\\' => {
                    out.push('\\');
                    i += 2;
                }
                b'\'' => {
                    out.push('\'');
                    i += 2;
                }
                b'"' => {
                    out.push('"');
                    i += 2;
                }
                _ => {
                    out.push('\\');
                    i += 1;
                }
            }
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

/// Unescape backslash sequences in an unquoted word.
fn unescape_word(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            out.push(bytes[i + 1] as char);
            i += 2;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

/// Extract all simple commands from an AST node (flattened).
pub fn extract_simple_commands(node: &BashNode) -> Vec<&SimpleCommand> {
    let mut commands = Vec::new();
    collect_simple_commands(node, &mut commands);
    commands
}

fn collect_simple_commands<'a>(node: &'a BashNode, commands: &mut Vec<&'a SimpleCommand>) {
    match node {
        BashNode::SimpleCommand(cmd) => commands.push(cmd),
        BashNode::Pipeline(nodes) => {
            for n in nodes {
                collect_simple_commands(n, commands);
            }
        }
        BashNode::List { left, right, .. } => {
            collect_simple_commands(left, commands);
            collect_simple_commands(right, commands);
        }
        BashNode::Subshell(inner) | BashNode::CommandSubstitution(inner) => {
            collect_simple_commands(inner, commands);
        }
        BashNode::Compound { .. } | BashNode::Assignment { .. } | BashNode::Raw(_) => {}
    }
}

/// Check if a command has any output redirections.
pub fn has_output_redirect(node: &BashNode) -> bool {
    for cmd in extract_simple_commands(node) {
        for r in &cmd.redirects {
            if matches!(
                r.operator,
                RedirectOperator::Write
                    | RedirectOperator::Append
                    | RedirectOperator::Both
                    | RedirectOperator::Clobber
            ) {
                return true;
            }
        }
    }
    false
}

/// Extract the argv (command name + args) from a simple command node.
///
/// Returns `None` for non-simple-command nodes.
pub fn extract_argv(node: &BashNode) -> Option<Vec<String>> {
    if let BashNode::SimpleCommand(cmd) = node {
        if cmd.command.is_empty() {
            return None;
        }
        let mut argv = vec![cmd.command.clone()];
        argv.extend(cmd.args.iter().cloned());
        Some(argv)
    } else {
        None
    }
}

#[cfg(test)]
#[path = "ast.test.rs"]
mod tests;
