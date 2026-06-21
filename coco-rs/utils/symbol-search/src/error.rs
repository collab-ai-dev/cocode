use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum SymbolSearchError {
    #[error("io error reading {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("unsupported language: {path}")]
    UnsupportedLanguage { path: PathBuf },

    #[error("tree-sitter tags configuration failed for {language}: {message}")]
    TagsConfiguration { language: String, message: String },

    #[error("tree-sitter tags generation failed: {message}")]
    TagsGeneration { message: String },
}
