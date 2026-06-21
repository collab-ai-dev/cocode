//! Read / write memory files (markdown + YAML frontmatter).
//!
//! Uses `coco-frontmatter` for YAML parsing — the previous hand-rolled
//! parser couldn't handle quoted values, multi-line strings, or escapes.

use std::path::Path;

use coco_frontmatter::FrontmatterValue;

use super::types::MemoryEntry;
use super::types::MemoryEntryType;
use super::types::MemoryFrontmatter;

/// Serialize a memory entry as a markdown file with YAML frontmatter.
///
/// Format:
///
/// ```markdown
/// ---
/// name: <name>
/// description: <one-line>
/// type: <user|feedback|project|reference>
/// ---
///
/// <content>
/// ```
pub fn format_entry_as_markdown(entry: &MemoryEntry) -> String {
    format!(
        "---\nname: {name}\ndescription: {desc}\ntype: {ty}\n---\n\n{body}\n",
        name = entry.name,
        desc = entry.description,
        ty = entry.memory_type.as_str(),
        body = entry.content.trim_end_matches('\n'),
    )
}

/// Parse a memory file's contents into a [`MemoryEntry`].
///
/// Returns `None` when frontmatter is missing, the `type` field is
/// missing or not one of the four canonical taxonomy strings, or when
/// `name` / `description` are missing. The caller decides whether to
/// drop the file or surface a warning — we never coerce to `User`.
pub fn parse_memory_entry(path: &Path, content: &str) -> Option<MemoryEntry> {
    let fm = parse_memory_frontmatter(content)?;
    let parsed = coco_frontmatter::parse(content);
    Some(MemoryEntry {
        name: fm.name,
        description: fm.description,
        memory_type: fm.memory_type,
        content: parsed.content,
        filename: path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string(),
        file_path: path.to_path_buf(),
    })
}

/// Parse just the frontmatter from `content`, useful for the scan path
/// where only the manifest fields are needed (no body load).
pub fn parse_memory_frontmatter(content: &str) -> Option<MemoryFrontmatter> {
    let parsed = coco_frontmatter::parse(content);
    let name = parsed.data.get("name").and_then(FrontmatterValue::as_str)?;
    let description = parsed
        .data
        .get("description")
        .and_then(FrontmatterValue::as_str)?;
    let ty_str = parsed.data.get("type").and_then(FrontmatterValue::as_str)?;
    let memory_type = MemoryEntryType::parse(ty_str)?;
    Some(MemoryFrontmatter {
        name: name.to_string(),
        description: description.to_string(),
        memory_type,
    })
}

#[cfg(test)]
#[path = "format.test.rs"]
mod tests;
