//! `/btw <question>` — open a local TUI sidechat.
//!
//! Runtime ownership lives in the TUI's local AppServer bridge. This command
//! module owns only syntax and the fallback text shown on surfaces that cannot
//! create local ephemeral child sessions.

/// Parsed `/btw` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtwRequest {
    pub question: String,
}

impl BtwRequest {
    pub const USAGE: &'static str = "Usage: /btw <your question> (or /btw --close)";

    pub fn parse(args: &str) -> Result<Self, &'static str> {
        let question = args.trim();
        if question.is_empty() {
            return Err(Self::USAGE);
        }
        Ok(Self {
            question: question.to_string(),
        })
    }

    pub fn is_close(&self) -> bool {
        self.question == "--close"
    }
}

/// Honest fallback for non-TUI command execution. The local TUI intercepts
/// `/btw` before invoking this handler.
pub fn handler(args: &str) -> String {
    match BtwRequest::parse(args) {
        Ok(_) => "/btw is available only in the interactive TUI.".to_string(),
        Err(usage) => usage.to_string(),
    }
}

#[cfg(test)]
#[path = "btw.test.rs"]
mod tests;
