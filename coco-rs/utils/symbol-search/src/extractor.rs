//! Symbol extraction using tree-sitter-tags.

use tree_sitter_tags::TagsContext;

use crate::SymbolKind;
use crate::error::SymbolSearchError;
use crate::languages::SymbolLanguage;

/// An extracted symbol tag.
#[derive(Debug, Clone)]
pub struct SymbolTag {
    /// Symbol name.
    pub name: String,
    /// Kind of symbol.
    pub kind: SymbolKind,
    /// Start line (1-indexed for display).
    pub line: i32,
    /// Whether this is a definition (vs reference).
    pub is_definition: bool,
}

/// Tag extractor using tree-sitter-tags.
pub struct SymbolExtractor {
    context: TagsContext,
}

impl Default for SymbolExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl SymbolExtractor {
    pub fn new() -> Self {
        Self {
            context: TagsContext::new(),
        }
    }

    /// Extract symbol tags from source code.
    pub fn extract(
        &mut self,
        source: &str,
        language: SymbolLanguage,
    ) -> Result<Vec<SymbolTag>, SymbolSearchError> {
        let config = language.tags_configuration()?;
        let source_bytes = source.as_bytes();

        let (tags, _errors) = self
            .context
            .generate_tags(&config, source_bytes, None)
            .map_err(|e| SymbolSearchError::TagsGeneration {
                message: format!("{e:?}"),
            })?;

        let mut result = Vec::new();

        for tag in tags {
            let tag = match tag {
                Ok(t) => t,
                Err(_) => continue,
            };

            let name_range = tag.name_range;
            let name = std::str::from_utf8(&source_bytes[name_range.start..name_range.end])
                .unwrap_or("")
                .to_string();

            if name.is_empty() {
                continue;
            }

            let syntax_type = config.syntax_type_name(tag.syntax_type_id);
            let line = source[..tag.range.start].lines().count() as i32 + 1;

            result.push(SymbolTag {
                name,
                kind: SymbolKind::from_syntax_type(syntax_type),
                line,
                is_definition: tag.is_definition,
            });
        }

        Ok(result)
    }

    /// Extract symbol tags from a file.
    pub fn extract_file(
        &mut self,
        path: &std::path::Path,
    ) -> Result<Vec<SymbolTag>, SymbolSearchError> {
        let source = std::fs::read_to_string(path).map_err(|e| SymbolSearchError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        let language = SymbolLanguage::from_path(path).ok_or_else(|| {
            SymbolSearchError::UnsupportedLanguage {
                path: path.to_path_buf(),
            }
        })?;
        self.extract(&source, language)
    }
}

#[cfg(test)]
#[path = "extractor.test.rs"]
mod tests;
