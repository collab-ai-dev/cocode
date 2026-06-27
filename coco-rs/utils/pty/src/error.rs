use std::io;

#[derive(Debug, thiserror::Error)]
pub enum PtyError {
    #[error("missing program for spawn")]
    MissingProgram,

    #[error(transparent)]
    Io(#[from] io::Error),

    #[error("portable-pty: {0}")]
    PortablePty(String),

    #[error("PTY handles lock poisoned")]
    PtyHandlesPoisoned,

    #[error("process is not attached to a PTY")]
    NotAPty,

    #[error("openpty failed: {0}")]
    OpenPty(io::Error),

    #[cfg(windows)]
    #[error("ConPTY: {0}")]
    ConPty(String),
}

pub type PtyResult<T> = std::result::Result<T, PtyError>;
