#[derive(Debug, thiserror::Error)]
pub enum FileSearchError {
    #[error("at least one search directory is required")]
    MissingSearchRoot,

    #[error("invalid override pattern: {pattern}")]
    InvalidOverride {
        pattern: String,
        #[source]
        source: ignore::Error,
    },

    #[error("failed to build override matcher")]
    OverrideBuild(#[source] ignore::Error),

    #[error("io error")]
    Io(#[from] std::io::Error),
}
