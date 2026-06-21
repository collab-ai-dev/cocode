//! Supported languages for symbol extraction.

use std::path::Path;

use tree_sitter_tags::TagsConfiguration;

use crate::error::SymbolSearchError;

/// Supported programming languages for symbol extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolLanguage {
    Rust,
    Go,
    Python,
    Java,
    TypeScript,
}

impl SymbolLanguage {
    /// Detect language from file extension.
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_lowercase().as_str() {
            "rs" => Some(Self::Rust),
            "go" => Some(Self::Go),
            "py" | "pyw" | "pyi" => Some(Self::Python),
            "java" => Some(Self::Java),
            "ts" | "tsx" | "js" | "jsx" | "mts" | "cts" => Some(Self::TypeScript),
            _ => None,
        }
    }

    /// Detect language from file path.
    pub fn from_path(path: &Path) -> Option<Self> {
        path.extension()
            .and_then(|e| e.to_str())
            .and_then(Self::from_extension)
    }

    /// Get tree-sitter language.
    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn tree_sitter_language(&self) -> tree_sitter::Language {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::Go => tree_sitter_go::LANGUAGE.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::Java => tree_sitter_java::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        }
    }

    /// Get tags configuration for this language.
    pub fn tags_configuration(&self) -> Result<TagsConfiguration, SymbolSearchError> {
        let language = self.tree_sitter_language();
        let query = self.tags_query();
        TagsConfiguration::new(language, query, "").map_err(|e| {
            SymbolSearchError::TagsConfiguration {
                language: format!("{self:?}"),
                message: e.to_string(),
            }
        })
    }

    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn tags_query(&self) -> &'static str {
        match self {
            Self::Rust => RUST_TAGS_QUERY,
            Self::Go => GO_TAGS_QUERY,
            Self::Python => PYTHON_TAGS_QUERY,
            Self::Java => JAVA_TAGS_QUERY,
            Self::TypeScript => TYPESCRIPT_TAGS_QUERY,
        }
    }
}

const RUST_TAGS_QUERY: &str = r#"
(function_item
  name: (identifier) @name) @definition.function

(function_signature_item
  name: (identifier) @name) @definition.function

(struct_item
  name: (type_identifier) @name) @definition.class

(enum_item
  name: (type_identifier) @name) @definition.class

(trait_item
  name: (type_identifier) @name) @definition.interface

(impl_item
  trait: (type_identifier)? @name
  type: (type_identifier) @name) @definition.class

(mod_item
  name: (identifier) @name) @definition.module

(const_item
  name: (identifier) @name) @definition.constant

(static_item
  name: (identifier) @name) @definition.constant

(type_item
  name: (type_identifier) @name) @definition.type

(macro_definition
  name: (identifier) @name) @definition.function
"#;

const GO_TAGS_QUERY: &str = r#"
(function_declaration
  name: (identifier) @name) @definition.function

(method_declaration
  name: (field_identifier) @name) @definition.method

(type_declaration
  (type_spec
    name: (type_identifier) @name
    type: (struct_type))) @definition.class

(type_declaration
  (type_spec
    name: (type_identifier) @name
    type: (interface_type))) @definition.interface

(type_declaration
  (type_spec
    name: (type_identifier) @name)) @definition.type

(const_declaration
  (const_spec
    name: (identifier) @name)) @definition.constant

(var_declaration
  (var_spec
    name: (identifier) @name)) @definition.constant
"#;

const PYTHON_TAGS_QUERY: &str = r#"
(function_definition
  name: (identifier) @name) @definition.function

(class_definition
  name: (identifier) @name) @definition.class

(decorated_definition
  definition: (function_definition
    name: (identifier) @name)) @definition.function

(decorated_definition
  definition: (class_definition
    name: (identifier) @name)) @definition.class
"#;

const JAVA_TAGS_QUERY: &str = r#"
(method_declaration
  name: (identifier) @name) @definition.method

(constructor_declaration
  name: (identifier) @name) @definition.method

(class_declaration
  name: (identifier) @name) @definition.class

(interface_declaration
  name: (identifier) @name) @definition.interface

(enum_declaration
  name: (identifier) @name) @definition.class

(field_declaration
  declarator: (variable_declarator
    name: (identifier) @name)) @definition.constant

(constant_declaration
  declarator: (variable_declarator
    name: (identifier) @name)) @definition.constant
"#;

const TYPESCRIPT_TAGS_QUERY: &str = r#"
(function_declaration
  name: (identifier) @name) @definition.function

(class_declaration
  name: (type_identifier) @name) @definition.class

(interface_declaration
  name: (type_identifier) @name) @definition.interface

(type_alias_declaration
  name: (type_identifier) @name) @definition.type

(enum_declaration
  name: (identifier) @name) @definition.class

(method_definition
  name: (property_identifier) @name) @definition.method

(lexical_declaration
  (variable_declarator
    name: (identifier) @name
    value: (arrow_function))) @definition.function
"#;

#[cfg(test)]
#[path = "languages.test.rs"]
mod tests;
