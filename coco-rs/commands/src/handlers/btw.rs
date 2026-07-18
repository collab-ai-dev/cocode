//! `/btw [question]` — open a local TUI sidechat.
//!
//! Runtime ownership lives in the TUI's local AppServer bridge. This command
//! module owns only syntax and the fallback text shown on surfaces that cannot
//! create local ephemeral child sessions.

/// Parsed `/btw` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BtwRequest {
    Open,
    OpenAndAsk { question: String },
}

impl BtwRequest {
    pub fn parse(args: &str) -> Self {
        let question = args.trim();
        if question.is_empty() {
            return Self::Open;
        }
        Self::OpenAndAsk {
            question: question.to_string(),
        }
    }
}

/// Honest fallback for non-TUI command execution. The local TUI intercepts
/// `/btw` before invoking this handler.
pub fn handler(args: &str) -> String {
    let _ = BtwRequest::parse(args);
    "/btw is available only in the interactive TUI.".to_string()
}

#[cfg(test)]
#[path = "btw.test.rs"]
mod tests;
