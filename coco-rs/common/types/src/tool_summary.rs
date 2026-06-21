//! Pure, UI-free per-tool input summarisation.
//!
//! Flattens a tool call's argument object into a short, human-readable
//! one-liner — the file path for `Read`, the command for `Bash`, the
//! description/prompt for `Agent`, the `pattern in path` for `Grep`, etc.
//!
//! This lives in `coco-types` (not `app/tui`) so producers below the UI
//! layer can reuse it: the swarm coordinator stamps
//! [`crate::TaskActivity::summary`] from here, and the TUI renders the
//! same text in the activity strip, the invocation header, and permission
//! prompts. One source of truth keeps those surfaces from drifting.

use std::str::FromStr;

use serde_json::Value;

use crate::MCP_TOOL_SEPARATOR;
use crate::ToolName;

/// Resolve a (possibly MCP-prefixed) tool name to its builtin [`ToolName`].
/// `mcp__server__tool` resolves on the trailing segment; unknown names
/// return `None`.
pub fn normalized_builtin_tool(tool_name: &str) -> Option<ToolName> {
    let normalized = tool_name
        .rsplit(MCP_TOOL_SEPARATOR)
        .next()
        .unwrap_or(tool_name);
    ToolName::from_str(normalized).ok()
}

/// One-line semantic summary of a tool call's input. Picks the salient
/// argument per tool (path / command / pattern / description); falls back
/// to a `key: value, …` object summary for unrecognised tools.
pub fn tool_input_summary(tool_name: &str, input: &Value) -> String {
    let Some(tool) = normalized_builtin_tool(tool_name) else {
        return object_summary(input);
    };
    if matches!(tool, ToolName::Bash | ToolName::PowerShell)
        && let Some(command) = input.get("command").and_then(Value::as_str)
    {
        return command.to_string();
    }

    match tool {
        ToolName::Glob => join_existing(input, &["pattern", "path"], " in "),
        ToolName::Grep => join_existing(input, &["pattern", "path"], " in "),
        ToolName::Read => read_target_preview(input),
        ToolName::Edit | ToolName::Write | ToolName::NotebookEdit => {
            scalar_value(input, "file_path")
                .or_else(|| scalar_value(input, "path"))
                .unwrap_or_default()
        }
        ToolName::WebFetch => scalar_value(input, "url").unwrap_or_default(),
        ToolName::WebSearch => scalar_value(input, "query").unwrap_or_default(),
        ToolName::Agent => scalar_value(input, "description")
            .or_else(|| scalar_value(input, "prompt"))
            .unwrap_or_default(),
        ToolName::ApplyPatch => input
            .get("patch")
            .and_then(Value::as_str)
            .map(|patch| apply_patch_target_paths(patch).join(", "))
            .filter(|paths| !paths.is_empty())
            .unwrap_or_default(),
        _ => object_summary(input),
    }
}

/// Extract target file paths from an apply_patch envelope's
/// `*** Add File:` / `*** Update File:` / `*** Delete File:` headers.
pub fn apply_patch_target_paths(patch: &str) -> Vec<&str> {
    const HEADERS: &[&str] = &["*** Add File: ", "*** Update File: ", "*** Delete File: "];
    patch
        .lines()
        .filter_map(|line| {
            HEADERS
                .iter()
                .find_map(|header| line.strip_prefix(header))
                .map(str::trim)
        })
        .filter(|p| !p.is_empty())
        .collect()
}

/// Read header preview: the path, plus a `· lines N-M` / `· from line N`
/// suffix when offset/limit are present (`lines {start}-{end}` when a limit
/// is set, else `from line {start}`).
fn read_target_preview(input: &Value) -> String {
    let path = scalar_value(input, "file_path")
        .or_else(|| scalar_value(input, "path"))
        .unwrap_or_default();
    let offset = input.get("offset").and_then(Value::as_i64);
    let limit = input.get("limit").and_then(Value::as_i64);
    if offset.is_none() && limit.is_none() {
        return path;
    }
    let start = offset.unwrap_or(1).max(1);
    let range = match limit {
        Some(limit) if limit > 0 => format!("lines {start}-{}", start + limit - 1),
        _ => format!("from line {start}"),
    };
    if path.is_empty() {
        range
    } else {
        format!("{path} · {range}")
    }
}

/// Multi-line input dump capped at `max_chars` — one `key: value` per
/// salient field. Used by the permission prompt where the user wants the
/// full picture rather than the single-line activity preview.
pub fn tool_input_multiline(tool_name: &str, input: &Value, max_chars: usize) -> String {
    let Some(tool) = normalized_builtin_tool(tool_name) else {
        return capped_lines(object_lines(input), max_chars);
    };
    if matches!(tool, ToolName::Bash | ToolName::PowerShell)
        && let Some(command) = input.get("command").and_then(Value::as_str)
    {
        return cap_single_line(command, max_chars);
    }
    // apply_patch: show the patch envelope itself (diff-like), not the
    // `{patch: …}` JSON wrapper it travels in.
    if matches!(tool, ToolName::ApplyPatch)
        && let Some(patch) = input.get("patch").and_then(Value::as_str)
    {
        return capped_lines(patch.lines().map(str::to_string).collect(), max_chars);
    }

    let keys: &[&str] = match tool {
        ToolName::Glob => &["path", "pattern"],
        ToolName::Grep => &["path", "pattern", "output_mode"],
        ToolName::Read => &["file_path", "offset", "limit"],
        ToolName::Edit => &["file_path", "old_string", "new_string"],
        ToolName::Write => &["file_path"],
        ToolName::NotebookEdit => &["file_path", "cell_id"],
        ToolName::WebFetch => &["url", "prompt"],
        ToolName::WebSearch => &["query"],
        ToolName::Agent => &["description", "subagent_type", "prompt"],
        _ => &[],
    };

    let mut lines = Vec::new();
    for key in keys {
        if let Some(value) = scalar_value(input, key) {
            lines.push(format!("{key}: {value}"));
        }
    }
    if lines.is_empty() {
        lines = object_lines(input);
    }

    capped_lines(lines, max_chars)
}

fn join_existing(input: &Value, keys: &[&str], separator: &str) -> String {
    keys.iter()
        .filter_map(|key| scalar_value(input, key))
        .collect::<Vec<_>>()
        .join(separator)
}

fn scalar_value(input: &Value, key: &str) -> Option<String> {
    value_to_display(input.get(key)?)
}

fn object_summary(input: &Value) -> String {
    let lines = object_lines(input);
    if !lines.is_empty() {
        return lines.join(", ");
    }
    match input {
        Value::Null => String::new(),
        other => value_to_display(other).unwrap_or_default(),
    }
}

fn object_lines(input: &Value) -> Vec<String> {
    let Some(obj) = input.as_object() else {
        return Vec::new();
    };

    obj.iter()
        .filter_map(|(key, value)| value_to_display(value).map(|value| format!("{key}: {value}")))
        .collect()
}

fn value_to_display(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::Array(values) => {
            let parts = values
                .iter()
                .filter_map(value_to_display)
                .collect::<Vec<_>>();
            (!parts.is_empty()).then(|| parts.join(", "))
        }
        Value::Object(_) => None,
    }
}

fn capped_lines(lines: Vec<String>, max_chars: usize) -> String {
    let mut out = String::new();
    let mut count = 0usize;

    for line in lines {
        let line = cap_single_line(&line, max_chars);
        let separator = usize::from(!out.is_empty());
        let line_len = line.chars().count();
        if count + separator + line_len > max_chars {
            if max_chars > 3 {
                while count + 3 > max_chars {
                    out.pop();
                    count = count.saturating_sub(1);
                }
                out.push_str("...");
            }
            return out;
        }
        if separator == 1 {
            out.push('\n');
            count += 1;
        }
        out.push_str(&line);
        count += line_len;
    }

    out
}

/// Collapse whitespace to single spaces and cap at `max_chars` (graphemes
/// approximated by chars), appending `...` when truncated.
pub fn cap_single_line(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }

    let mut out = String::new();
    let mut pending_space = false;
    let mut count = 0usize;
    for chunk in text.split_whitespace() {
        let space = if out.is_empty() || !pending_space {
            0
        } else {
            1
        };
        let chunk_len = chunk.chars().count();
        if count + space + chunk_len > max_chars {
            if max_chars > 3 {
                while count + 3 > max_chars {
                    out.pop();
                    count = count.saturating_sub(1);
                }
                out.push_str("...");
            }
            return out;
        }
        if space == 1 {
            out.push(' ');
            count += 1;
        }
        out.push_str(chunk);
        count += chunk_len;
        pending_space = true;
    }

    out
}

/// Best-effort extraction of the primary argument's (possibly partial) string
/// value from an *incomplete* tool-call input JSON buffer, for the live
/// activity-strip preview while the model streams arguments token by token.
///
/// Returns `None` until the opening quote of the value has arrived, then the
/// decoded value so far (which grows as more deltas land). Tolerant of the
/// half-written tail — a trailing `\` or unterminated string is fine. `None`
/// for tools without a meaningful primary string field (or unrecognised tools).
pub fn partial_primary_arg(tool_name: &str, partial_json: &str) -> Option<String> {
    // The single salient argument key whose streamed value drives the preview.
    let key = match normalized_builtin_tool(tool_name)? {
        ToolName::Bash | ToolName::PowerShell => "command",
        ToolName::Read | ToolName::Edit | ToolName::Write | ToolName::NotebookEdit => "file_path",
        ToolName::Grep | ToolName::Glob => "pattern",
        ToolName::WebFetch => "url",
        ToolName::WebSearch => "query",
        ToolName::Agent => "description",
        _ => return None,
    };
    partial_json_string_field(partial_json, key)
}

/// Scan a (possibly truncated) JSON buffer for `"key": "<value…>"` and return
/// the decoded value read so far. Handles the common escape sequences; stops
/// at the closing quote or end of buffer.
fn partial_json_string_field(buf: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let after_key = buf.find(&needle)? + needle.len();
    let rest = &buf[after_key..];
    let colon = rest.find(':')?;
    let mut chars = rest[colon + 1..].chars();
    // Skip whitespace up to the opening quote.
    let mut next = chars.next();
    while matches!(next, Some(c) if c.is_whitespace()) {
        next = chars.next();
    }
    if next != Some('"') {
        // Opening quote not yet streamed (or value is non-string).
        return None;
    }
    let mut out = String::new();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('b') => out.push('\u{8}'),
                Some('f') => out.push('\u{c}'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('/') => out.push('/'),
                Some('u') => {
                    // `\uXXXX` — read exactly 4 hex digits. A short tail means
                    // the escape is split across deltas: stop and let the rest
                    // arrive next time. Surrogates / invalid scalars fall back
                    // to U+FFFD (cosmetic preview, no round-trip requirement).
                    let mut code: u32 = 0;
                    let mut digits = 0;
                    for _ in 0..4 {
                        let Some(d) = chars.next().and_then(|h| h.to_digit(16)) else {
                            break;
                        };
                        code = code * 16 + d;
                        digits += 1;
                    }
                    if digits == 4 {
                        out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                    }
                }
                // Unknown escape: drop the sequence (JSON has no other escapes)
                // rather than emit a literal `\x`.
                Some(_) => {}
                // Trailing backslash mid-stream — value continues next delta.
                None => break,
            },
            '"' => break, // Closing quote: value complete.
            other => out.push(other),
        }
    }
    Some(out)
}

#[cfg(test)]
#[path = "tool_summary.test.rs"]
mod tests;
