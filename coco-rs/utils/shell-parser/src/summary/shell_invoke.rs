//! Shell invocation helpers ported from codex-rs/shell-command/src/bash.rs.
//!
//! Functions for extracting and parsing shell invocations (`bash -lc "..."`)
//! including heredoc detection. Reuses tree-sitter functions from `crate::parser`.

use std::path::PathBuf;

use tree_sitter::Node;
use tree_sitter::Tree;

use crate::parser::ShellType;
use crate::parser::detect_shell_type;

/// Parse the provided bash source using tree-sitter-bash, returning a Tree on
/// success or None if parsing failed.
pub fn try_parse_shell(shell_lc_arg: &str) -> Option<Tree> {
    let mut parser = crate::parser::ShellParser::new();
    let parsed = parser.parse(shell_lc_arg);
    parsed.tree().cloned()
}

/// Re-export from parser for use by other summary modules.
pub fn try_parse_word_only_commands_sequence(tree: &Tree, src: &str) -> Option<Vec<Vec<String>>> {
    crate::parser::try_parse_word_only_commands_sequence(tree, src)
}

pub fn extract_bash_command(command: &[String]) -> Option<(&str, &str)> {
    let [shell, flag, script] = command else {
        return None;
    };
    if !matches!(flag.as_str(), "-lc" | "-c")
        || !matches!(
            detect_shell_type(&PathBuf::from(shell)),
            ShellType::Zsh | ShellType::Bash | ShellType::Sh
        )
    {
        return None;
    }
    Some((shell, script))
}

/// Returns the sequence of plain commands within a `bash -lc "..."` or
/// `zsh -lc "..."` invocation when the script only contains word-only commands
/// joined by safe operators.
pub fn parse_shell_lc_plain_commands(command: &[String]) -> Option<Vec<Vec<String>>> {
    let (_, script) = extract_bash_command(command)?;
    let tree = try_parse_shell(script)?;
    try_parse_word_only_commands_sequence(&tree, script)
}

/// Returns the parsed argv for a single shell command in a here-doc style
/// script (`<<`), as long as the script contains exactly one command node.
#[allow(dead_code)]
pub fn parse_shell_lc_single_command_prefix(command: &[String]) -> Option<Vec<String>> {
    let (_, script) = extract_bash_command(command)?;
    let tree = try_parse_shell(script)?;
    let root = tree.root_node();
    if root.has_error() {
        return None;
    }
    if !has_named_descendant_kind(root, "heredoc_redirect") {
        return None;
    }

    let command_node = find_single_command_node(root)?;
    parse_heredoc_command_words(command_node, script)
}

fn parse_heredoc_command_words(cmd: Node<'_>, src: &str) -> Option<Vec<String>> {
    if cmd.kind() != "command" {
        return None;
    }

    let mut words = Vec::new();
    let mut cursor = cmd.walk();
    for child in cmd.named_children(&mut cursor) {
        match child.kind() {
            "command_name" => {
                let word_node = child.named_child(0)?;
                if !matches!(word_node.kind(), "word" | "number")
                    || !is_literal_word_or_number(word_node)
                {
                    return None;
                }
                words.push(word_node.utf8_text(src.as_bytes()).ok()?.to_owned());
            }
            "word" | "number" => {
                if !is_literal_word_or_number(child) {
                    return None;
                }
                words.push(child.utf8_text(src.as_bytes()).ok()?.to_owned());
            }
            // Allow shell constructs that attach IO to a single command without
            // changing argv matching semantics for the executable prefix.
            "variable_assignment" | "comment" => {}
            kind if is_allowed_heredoc_attachment_kind(kind) => {}
            _ => return None,
        }
    }

    if words.is_empty() { None } else { Some(words) }
}

fn is_literal_word_or_number(node: Node<'_>) -> bool {
    if !matches!(node.kind(), "word" | "number") {
        return false;
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor).next().is_none()
}

fn is_allowed_heredoc_attachment_kind(kind: &str) -> bool {
    matches!(
        kind,
        "heredoc_body"
            | "simple_heredoc_body"
            | "heredoc_redirect"
            | "herestring_redirect"
            | "file_redirect"
            | "redirected_statement"
    )
}

fn find_single_command_node(root: Node<'_>) -> Option<Node<'_>> {
    let mut stack = vec![root];
    let mut single_command = None;
    while let Some(node) = stack.pop() {
        if node.kind() == "command" {
            if single_command.is_some() {
                return None;
            }
            single_command = Some(node);
        }

        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            stack.push(child);
        }
    }
    single_command
}

fn has_named_descendant_kind(node: Node<'_>, kind: &str) -> bool {
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        if current.kind() == kind {
            return true;
        }
        let mut cursor = current.walk();
        for child in current.named_children(&mut cursor) {
            stack.push(child);
        }
    }
    false
}

#[cfg(test)]
#[path = "shell_invoke.test.rs"]
mod tests;
