//! Code-specific tokenizer for BM25 search.
//!
//! Handles code-specific patterns like:
//! - snake_case → [snake, case]
//! - camelCase → [camel, case]
//! - foo.bar → [foo, bar]
//! - foo::bar → [foo, bar]
//! - foo->bar → [foo, bar]

use bm25::Tokenizer;
use once_cell::sync::Lazy;
use regex::Regex;

/// Code-specific tokenizer that understands programming naming conventions.
///
/// Splits identifiers by:
/// - Underscores (snake_case)
/// - camelCase boundaries
/// - Dots, colons, arrows (member access)
/// - Standard whitespace and punctuation
#[derive(Debug, Clone, Default)]
pub struct CodeTokenizer {
    /// Minimum token length to include (filters noise)
    min_token_len: usize,
    /// Whether to lowercase tokens
    lowercase: bool,
}

impl CodeTokenizer {
    /// Create a new code tokenizer with default settings.
    pub fn new() -> Self {
        Self {
            min_token_len: 2,
            lowercase: true,
        }
    }

    /// Create a code tokenizer with custom settings.
    pub fn with_config(min_token_len: usize, lowercase: bool) -> Self {
        Self {
            min_token_len,
            lowercase,
        }
    }

    /// Preprocess code text by splitting naming conventions.
    fn preprocess_code(&self, text: &str) -> String {
        // 1. Replace common code separators with spaces
        static SEPARATOR_RE: Lazy<Regex> =
            Lazy::new(|| Regex::new(r"(::|\->|\.|\-|/|\\)").expect("invalid regex"));
        let text = SEPARATOR_RE.replace_all(text, " ");

        // 2. Split snake_case: foo_bar → foo bar
        static SNAKE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"_+").expect("invalid regex"));
        let text = SNAKE_RE.replace_all(&text, " ");

        // 3. Split camelCase and PascalCase: fooBar → foo Bar, FooBar → Foo Bar
        // Insert space before uppercase letters that follow lowercase letters
        static CAMEL_RE: Lazy<Regex> =
            Lazy::new(|| Regex::new(r"([a-z])([A-Z])").expect("invalid regex"));
        let text = CAMEL_RE.replace_all(&text, "$1 $2");

        // 4. Split sequences like HTTPServer → HTTP Server (uppercase followed by uppercase+lowercase)
        static ACRONYM_RE: Lazy<Regex> =
            Lazy::new(|| Regex::new(r"([A-Z]+)([A-Z][a-z])").expect("invalid regex"));
        let text = ACRONYM_RE.replace_all(&text, "$1 $2");

        // 5. Remove common code symbols and punctuation
        static SYMBOL_RE: Lazy<Regex> =
            Lazy::new(|| Regex::new(r#"[(){}\[\]<>;,=+*&|!?@#$%^~`"']"#).expect("invalid regex"));
        let text = SYMBOL_RE.replace_all(&text, " ");

        text.into_owned()
    }
}

impl Tokenizer for CodeTokenizer {
    fn tokenize(&self, input_text: &str) -> Vec<String> {
        let preprocessed = self.preprocess_code(input_text);

        preprocessed
            .split_whitespace()
            .filter_map(|token| {
                // Apply lowercase if configured
                let token = if self.lowercase {
                    token.to_lowercase()
                } else {
                    token.to_string()
                };

                // Filter by minimum length
                if token.len() >= self.min_token_len {
                    Some(token)
                } else {
                    None
                }
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "code_tokenizer.test.rs"]
mod tests;
