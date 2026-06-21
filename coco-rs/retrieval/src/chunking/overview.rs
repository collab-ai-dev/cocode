//! Smart overview chunk generation.
//!
//! Generates structure overview chunks for classes/modules with method signatures
//! collapsed to `{ ... }`, similar to Continue's getSmartCollapsedChunks.

use crate::tags::CodeTag;
use crate::tags::TagKind;
use crate::types::ChunkSpan;

/// Configuration for overview chunk generation.
#[derive(Debug, Clone)]
pub struct OverviewConfig {
    /// Minimum number of methods for a class to generate an overview.
    pub min_methods: usize,
    /// Maximum overview size in characters.
    pub max_size: usize,
}

impl Default for OverviewConfig {
    fn default() -> Self {
        Self {
            min_methods: 2,
            max_size: 4096,
        }
    }
}

/// Generate overview chunks for classes/structs in the source.
///
/// For each class/struct with enough methods, generates a chunk showing:
/// - The class definition header
/// - All method signatures with bodies collapsed to `{ ... }`
///
/// This allows semantic search to find "what methods does UserService have"
/// without returning the full implementation of each method.
pub fn generate_overview_chunks(
    source: &str,
    tags: &[CodeTag],
    config: &OverviewConfig,
) -> Vec<ChunkSpan> {
    let mut overviews = Vec::new();
    let lines: Vec<&str> = source.lines().collect();

    // Find all container types (class, struct, interface, module)
    let containers: Vec<&CodeTag> = tags
        .iter()
        .filter(|t| {
            matches!(
                t.kind,
                TagKind::Class | TagKind::Struct | TagKind::Interface | TagKind::Module
            )
        })
        .collect();

    for container in containers {
        // Find all methods/functions that belong to this container
        let methods: Vec<&CodeTag> = tags
            .iter()
            .filter(|t| {
                matches!(t.kind, TagKind::Function | TagKind::Method)
                    && t.start_line > container.start_line
                    && t.end_line <= container.end_line
            })
            .collect();

        if methods.len() < config.min_methods {
            continue;
        }

        // Build overview content
        let mut content = String::new();

        // Add container header (lines from start to first method or first brace)
        let header_end = methods
            .first()
            .map(|m| m.start_line)
            .unwrap_or(container.end_line);

        for i in container.start_line..header_end {
            if let Some(line) = lines.get(i as usize) {
                content.push_str(line);
                content.push('\n');
            }
        }

        // Add each method signature with collapsed body
        for method in &methods {
            if let Some(collapsed) = collapse_method_to_signature(source, method, &lines) {
                content.push_str(&collapsed);
                content.push('\n');
            }
        }

        // Add closing brace if container has one
        if let Some(last_line) = lines.get(container.end_line as usize) {
            let trimmed = last_line.trim();
            if trimmed == "}" || trimmed == "};" {
                content.push_str(last_line);
                content.push('\n');
            }
        }

        // Truncate if too large
        if content.len() > config.max_size {
            content.truncate(config.max_size);
            content.push_str("\n// ... truncated");
        }

        overviews.push(ChunkSpan {
            content,
            start_line: container.start_line,
            end_line: container.end_line,
            is_overview: true,
        });
    }

    overviews
}

/// Collapse a method to its signature with `{ ... }` or `...` for Python.
fn collapse_method_to_signature(_source: &str, tag: &CodeTag, lines: &[&str]) -> Option<String> {
    // Extract the method's source text
    let start = tag.start_line as usize;
    let end = tag.end_line as usize;

    if start >= lines.len() {
        return None;
    }

    // Determine indentation from first line
    let first_line = lines.get(start).unwrap_or(&"");
    let indent = first_line.len() - first_line.trim_start().len();
    let indent_str: String = first_line.chars().take(indent).collect();

    // Find the opening brace position
    let mut method_text = String::new();
    for line in &lines[start..=end.min(lines.len() - 1)] {
        method_text.push_str(line);
        method_text.push('\n');
    }

    // Try brace-based languages first (Rust, Go, Java, JS, etc.)
    if let Some(brace_pos) = method_text.find('{') {
        // Get signature (everything before the brace)
        let signature = method_text[..brace_pos].trim_end();
        return Some(format!("{}{} {{ ... }}", indent_str, signature.trim()));
    }

    // Python-style: look for `:` at end of first line (def foo(): or def foo(args):)
    let first_trimmed = first_line.trim();
    if (first_trimmed.starts_with("def ") || first_trimmed.starts_with("async def "))
        && first_trimmed.ends_with(':')
    {
        // Remove the colon and add collapsed body indicator
        let sig = &first_trimmed[..first_trimmed.len() - 1];
        return Some(format!("{}{}: ...", indent_str, sig.trim()));
    }

    // For other cases (abstract methods, declarations), return first line
    Some(first_line.to_string())
}

/// Check if an overview should be generated for a container.
pub fn should_generate_overview(tags: &[CodeTag], container: &CodeTag, min_methods: usize) -> bool {
    let method_count = tags
        .iter()
        .filter(|t| {
            matches!(t.kind, TagKind::Function | TagKind::Method)
                && t.start_line > container.start_line
                && t.end_line <= container.end_line
        })
        .count();

    method_count >= min_methods
}

#[cfg(test)]
#[path = "overview.test.rs"]
mod tests;
