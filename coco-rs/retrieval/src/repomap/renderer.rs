//! Tree renderer for repo map output.
//!
//! Formats ranked symbols as a tree structure with file paths
//! and line numbers for LLM context.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;

use super::RankedSymbol;

/// Maximum line length before truncation (for minified JS, etc.)
const MAX_LINE_LENGTH: usize = 100;

/// Tree renderer for repo map output.
pub struct TreeRenderer {
    /// Include line numbers in output
    show_line_numbers: bool,
    /// Include signatures in output
    show_signatures: bool,
}

impl TreeRenderer {
    /// Create a new tree renderer with default settings.
    pub fn new() -> Self {
        Self {
            show_line_numbers: true,
            show_signatures: true,
        }
    }

    /// Create a renderer with custom settings.
    #[cfg(test)]
    pub fn with_options(show_line_numbers: bool, show_signatures: bool) -> Self {
        Self {
            show_line_numbers,
            show_signatures,
        }
    }

    /// Render ranked symbols as a tree.
    ///
    /// # Arguments
    /// * `symbols` - Ranked symbols sorted by rank descending
    /// * `chat_files` - Files in chat context (highlighted)
    /// * `count` - Number of symbols to include
    /// * `workspace_root` - Workspace root for relative path display
    ///
    /// # Returns
    /// A tuple of (rendered content, set of rendered file paths)
    pub fn render(
        &self,
        symbols: &[RankedSymbol],
        chat_files: &HashSet<String>,
        count: i32,
        _workspace_root: &Path,
    ) -> (String, HashSet<String>) {
        // Take top N symbols
        let symbols_to_render = &symbols[..symbols.len().min(count as usize)];

        // Group by file
        let mut file_symbols: HashMap<String, Vec<&RankedSymbol>> = HashMap::new();
        for sym in symbols_to_render {
            file_symbols
                .entry(sym.filepath.clone())
                .or_default()
                .push(sym);
        }

        // Sort files by their highest-ranked symbol
        let mut file_order: Vec<(String, f64)> = file_symbols
            .iter()
            .map(|(path, syms)| {
                let max_rank = syms.iter().map(|s| s.rank).fold(0.0_f64, f64::max);
                (path.clone(), max_rank)
            })
            .collect();
        file_order.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Track rendered files
        let mut rendered_files: HashSet<String> = HashSet::new();

        // Render output
        let mut output = String::new();

        for (filepath, _rank) in file_order {
            let syms = match file_symbols.get(&filepath) {
                Some(s) => s,
                None => continue,
            };

            // Track this file as rendered
            rendered_files.insert(filepath.clone());

            // File header (highlight chat files)
            let is_chat_file = chat_files.contains(&filepath);
            if is_chat_file {
                output.push_str(&format!("{}:  [chat]\n", filepath));
            } else {
                output.push_str(&format!("{}:\n", filepath));
            }

            // Sort symbols by line number within file
            let mut sorted_syms: Vec<&&RankedSymbol> = syms.iter().collect();
            sorted_syms.sort_by_key(|s| s.tag.start_line);

            // Render each symbol
            for sym in sorted_syms {
                self.render_symbol(&mut output, sym);
            }

            output.push('\n');
        }

        // Truncate long lines (e.g., minified JS)
        (Self::truncate_lines(output.trim_end()), rendered_files)
    }

    /// Render symbols without file context (for token counting).
    pub fn render_symbols(&self, symbols: &[RankedSymbol], count: i32) -> String {
        if symbols.is_empty() || count <= 0 {
            return String::new();
        }

        let mut output = String::new();
        let symbols_to_render = &symbols[..symbols.len().min(count as usize)];

        // Group by filepath
        let mut current_file: Option<String> = None;

        for sym in symbols_to_render {
            // Add file header if changed
            if current_file.as_ref() != Some(&sym.filepath) {
                if current_file.is_some() {
                    output.push('\n');
                }
                output.push_str(&format!("{}:\n", sym.filepath));
                current_file = Some(sym.filepath.clone());
            }

            self.render_symbol(&mut output, sym);
        }

        // Truncate long lines
        Self::truncate_lines(&output)
    }

    /// Render a single symbol.
    fn render_symbol(&self, output: &mut String, sym: &RankedSymbol) {
        let tag = &sym.tag;

        if self.show_line_numbers {
            output.push_str(&format!("│{:>4}: ", tag.start_line));
        } else {
            output.push_str("│  ");
        }

        if let Some(sig) = &tag.signature
            && self.show_signatures
        {
            output.push_str(sig);
        } else {
            // Fallback to kind + name
            output.push_str(&format!("{:?} {}", tag.kind, tag.name));
        }

        output.push('\n');
    }

    /// Truncate lines that exceed MAX_LINE_LENGTH.
    fn truncate_lines(output: &str) -> String {
        output
            .lines()
            .map(|line| {
                if line.len() > MAX_LINE_LENGTH {
                    // Use char boundary to avoid panic on multi-byte UTF-8 characters
                    let truncated: String = line.chars().take(MAX_LINE_LENGTH - 3).collect();
                    format!("{truncated}...")
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Default for TreeRenderer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "renderer.test.rs"]
mod tests;
