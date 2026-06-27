//! Frontmatter parser for markdown files.
//!
//! Extracts and parses YAML frontmatter between `---` delimiters.
//! Used by skills, commands, agents, memory files, and output styles.
//!
//! Backed by [`serde_yml`] for full YAML compliance — supports nested
//! mappings, sequences of mappings, multi-line strings, and the
//! standard YAML scalar types. Earlier versions used a hand-written
//! flat parser that could not represent the inline `mcpServers:
//! {name: config}` or nested `hooks: {PreToolUse: [...]}` shapes
//! agent frontmatter uses.

use std::collections::BTreeMap;
use std::collections::HashMap;

/// Parsed frontmatter result.
#[derive(Debug, Clone)]
pub struct Frontmatter {
    /// Parsed key-value data from YAML frontmatter.
    pub data: HashMap<String, FrontmatterValue>,
    /// Markdown content after the frontmatter block.
    pub content: String,
}

/// A value in the frontmatter YAML.
#[derive(Debug, Clone, PartialEq)]
pub enum FrontmatterValue {
    /// String value.
    String(String),
    /// Boolean value.
    Bool(bool),
    /// Integer value.
    Int(i64),
    /// Floating-point value.
    Float(f64),
    /// List of arbitrary values.
    /// String-only lists still pass through `as_string_list`.
    Sequence(Vec<FrontmatterValue>),
    /// Nested object (mapping of string keys to arbitrary values). Used for
    /// inline `mcpServers: {name: config}` and `hooks:` shapes.
    Mapping(BTreeMap<String, FrontmatterValue>),
    /// Null/empty value (key with no value, e.g., `key:`).
    Null,
}

impl FrontmatterValue {
    /// Get as string, if it is one.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            FrontmatterValue::String(s) => Some(s),
            _ => None,
        }
    }

    /// Get as bool, if it is one.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            FrontmatterValue::Bool(b) => Some(*b),
            FrontmatterValue::String(s) => match s.to_lowercase().as_str() {
                "true" | "yes" | "1" => Some(true),
                "false" | "no" | "0" => Some(false),
                _ => None,
            },
            _ => None,
        }
    }

    /// Get as integer, if it is one.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            FrontmatterValue::Int(n) => Some(*n),
            _ => None,
        }
    }

    /// Get as string list, if it is one. Single strings become a 1-element list.
    /// Sequences containing non-string items are filtered down to the strings only.
    pub fn as_string_list(&self) -> Option<Vec<&str>> {
        match self {
            FrontmatterValue::Sequence(items) => {
                Some(items.iter().filter_map(FrontmatterValue::as_str).collect())
            }
            FrontmatterValue::String(s) => Some(vec![s.as_str()]),
            _ => None,
        }
    }

    /// Get as nested mapping, if it is one.
    pub fn as_mapping(&self) -> Option<&BTreeMap<String, FrontmatterValue>> {
        match self {
            FrontmatterValue::Mapping(m) => Some(m),
            _ => None,
        }
    }

    /// Get as raw sequence, if it is one. Useful when the items can be
    /// mixed (`AgentMcpServerSpec` accepts `string | mapping`).
    pub fn as_sequence(&self) -> Option<&[FrontmatterValue]> {
        match self {
            FrontmatterValue::Sequence(items) => Some(items),
            _ => None,
        }
    }

    /// Convert to `serde_json::Value` so callers (e.g.
    /// `coco_hooks::load_hooks_from_config`) can consume nested
    /// shapes without a second parser. Used for the hooks-from-
    /// frontmatter pipeline.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            FrontmatterValue::String(s) => serde_json::Value::String(s.clone()),
            FrontmatterValue::Bool(b) => serde_json::Value::Bool(*b),
            FrontmatterValue::Int(n) => serde_json::json!(n),
            FrontmatterValue::Float(f) => serde_json::json!(f),
            FrontmatterValue::Null => serde_json::Value::Null,
            FrontmatterValue::Sequence(items) => {
                serde_json::Value::Array(items.iter().map(FrontmatterValue::to_json).collect())
            }
            FrontmatterValue::Mapping(m) => {
                let obj: serde_json::Map<String, serde_json::Value> =
                    m.iter().map(|(k, v)| (k.clone(), v.to_json())).collect();
                serde_json::Value::Object(obj)
            }
        }
    }
}

/// Parse markdown file content, extracting YAML frontmatter.
///
/// Frontmatter is delimited by `---` on its own line at the start of the file.
/// Returns the parsed data and the remaining markdown content.
///
/// ```
/// # use coco_frontmatter::parse;
/// let md = "---\ntitle: Hello\n---\n# Body";
/// let fm = parse(md);
/// assert_eq!(fm.data.get("title").unwrap().as_str(), Some("Hello"));
/// assert_eq!(fm.content.trim(), "# Body");
/// ```
pub fn parse(input: &str) -> Frontmatter {
    let trimmed = input.trim_start();

    // Must start with ---
    if !trimmed.starts_with("---") {
        return Frontmatter {
            data: HashMap::new(),
            content: input.to_string(),
        };
    }

    // Find the closing ---
    let after_first = &trimmed[3..];
    let after_first = after_first.strip_prefix('\n').unwrap_or(after_first);

    // Empty frontmatter (`---\n---\n…`): the closer sits at index 0 with no
    // leading newline. The general `find("\n---")` lookup misses that, so
    // handle the zero-length block explicitly.
    if let Some(rest) = after_first.strip_prefix("---") {
        let body = rest
            .strip_prefix('\n')
            .or_else(|| rest.strip_prefix("\r\n"))
            .unwrap_or(rest);
        return Frontmatter {
            data: HashMap::new(),
            content: body.to_string(),
        };
    }

    if let Some(end_pos) = after_first.find("\n---") {
        let yaml_block = &after_first[..end_pos];
        let content_start = end_pos + 4; // skip \n---
        let content = after_first[content_start..]
            .strip_prefix('\n')
            .unwrap_or(&after_first[content_start..]);

        let data = parse_yaml_block(yaml_block);

        Frontmatter {
            data,
            content: content.to_string(),
        }
    } else {
        // No closing delimiter — treat entire content as body
        Frontmatter {
            data: HashMap::new(),
            content: input.to_string(),
        }
    }
}

/// Parse a YAML block via `serde_yml`. On parse failure, retries once
/// after auto-quoting values that contain YAML special characters
/// (e.g. globs like `*.{ts,tsx}`, where `*` is a YAML alias indicator
/// and `{}` opens a flow mapping).
///
/// Falls through to an empty map when even the retry fails (malformed
/// frontmatter doesn't poison the body).
fn parse_yaml_block(yaml: &str) -> HashMap<String, FrontmatterValue> {
    let trimmed = yaml.trim();
    if trimmed.is_empty() {
        return HashMap::new();
    }
    let value: serde_yml::Value = match serde_yml::from_str(yaml) {
        Ok(v) => v,
        Err(_) => match serde_yml::from_str(&quote_problematic_values(yaml)) {
            Ok(v) => v,
            Err(_) => return HashMap::new(),
        },
    };
    let mapping = match value {
        serde_yml::Value::Mapping(m) => m,
        _ => return HashMap::new(),
    };
    mapping
        .into_iter()
        .filter_map(|(k, v)| {
            let key = match k {
                serde_yml::Value::String(s) => s,
                serde_yml::Value::Number(n) => n.to_string(),
                serde_yml::Value::Bool(b) => b.to_string(),
                _ => return None,
            };
            Some((key, yaml_to_frontmatter_value(v)))
        })
        .collect()
}

/// Pre-process YAML text by wrapping values that contain YAML special
/// characters in double quotes, keeping glob patterns like `*.{ts,tsx}`
/// from being interpreted as
/// anchors or flow-mappings.
///
/// Only top-level `key: value` lines are rewritten; indented lines,
/// list items, and block-scalar headers pass through untouched so YAML
/// list / nested-mapping syntax keeps working.
fn quote_problematic_values(yaml: &str) -> String {
    let mut out = String::with_capacity(yaml.len() + 16);
    for (i, line) in yaml.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        match maybe_quote_line(line) {
            Some(rewritten) => out.push_str(&rewritten),
            None => out.push_str(line),
        }
    }
    if yaml.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn maybe_quote_line(line: &str) -> Option<String> {
    // Skip indented lines (sequence items, nested mappings).
    if line.starts_with(' ') || line.starts_with('\t') {
        return None;
    }

    let colon = line.find(':')?;
    let key = &line[..colon];
    if key.is_empty() || !key.chars().all(is_simple_key_char) {
        return None;
    }

    let after_colon = &line[colon + 1..];
    // Need at least one separator space — `key:value` without space is not
    // a YAML mapping line we want to touch.
    let value = after_colon.strip_prefix(' ')?.trim_end();
    if value.is_empty() {
        return None;
    }

    // Already quoted — leave alone.
    if (value.starts_with('"') && value.ends_with('"'))
        || (value.starts_with('\'') && value.ends_with('\''))
    {
        return None;
    }

    if !needs_quoting(value) {
        return None;
    }

    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    Some(format!("{key}: \"{escaped}\""))
}

fn is_simple_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

/// Returns true when the value contains characters that have special YAML
/// meaning at the start of a scalar or anywhere in a flow context.
/// Flags `': '` (nested mappings); bare `:` is fine for times like `12:34`.
fn needs_quoting(value: &str) -> bool {
    if value.contains(": ") {
        return true;
    }
    value.chars().any(|c| {
        matches!(
            c,
            '{' | '}' | '[' | ']' | '*' | '&' | '#' | '!' | '|' | '>' | '%' | '@' | '`'
        )
    })
}

fn yaml_to_frontmatter_value(value: serde_yml::Value) -> FrontmatterValue {
    match value {
        serde_yml::Value::Null => FrontmatterValue::Null,
        serde_yml::Value::Bool(b) => FrontmatterValue::Bool(b),
        serde_yml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                FrontmatterValue::Int(i)
            } else if let Some(f) = n.as_f64() {
                FrontmatterValue::Float(f)
            } else {
                FrontmatterValue::String(n.to_string())
            }
        }
        serde_yml::Value::String(s) => FrontmatterValue::String(s),
        serde_yml::Value::Sequence(seq) => {
            FrontmatterValue::Sequence(seq.into_iter().map(yaml_to_frontmatter_value).collect())
        }
        serde_yml::Value::Mapping(m) => {
            let map: BTreeMap<String, FrontmatterValue> = m
                .into_iter()
                .filter_map(|(k, v)| {
                    let key = match k {
                        serde_yml::Value::String(s) => s,
                        serde_yml::Value::Number(n) => n.to_string(),
                        serde_yml::Value::Bool(b) => b.to_string(),
                        _ => return None,
                    };
                    Some((key, yaml_to_frontmatter_value(v)))
                })
                .collect();
            FrontmatterValue::Mapping(map)
        }
        serde_yml::Value::Tagged(tagged) => yaml_to_frontmatter_value(tagged.value),
    }
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
