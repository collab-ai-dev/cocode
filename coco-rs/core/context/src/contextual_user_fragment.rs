//! Bounded user-fragment contracts shared by prompt-assembly features.

use coco_messages::estimate_text_tokens;

/// A bounded user-role context fragment inserted into a prompt exactly once.
/// It may be transcript-visible or prompt-only; implementors render
/// deterministic, budget-bounded text.
pub trait ContextualUserFragment {
    /// Render the fragment's text.
    fn render(&self) -> String;

    /// Estimated token cost of the rendered text. Reuses coco-messages so the
    /// estimate matches history accounting.
    fn estimated_tokens(&self) -> i64 {
        estimate_text_tokens(&self.render())
    }
}

/// Bounded model-facing nudge used to repair a malformed terminal response.
/// Query owns retry policy; context owns the prompt-fragment budget contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalRecoveryNudgeFragment {
    text: String,
}

impl TerminalRecoveryNudgeFragment {
    pub const MAX_BYTES: usize = 4_096;

    pub fn new(text: &str) -> Self {
        Self {
            text: coco_utils_string::take_bytes_at_char_boundary(text, Self::MAX_BYTES).to_string(),
        }
    }
}

impl ContextualUserFragment for TerminalRecoveryNudgeFragment {
    fn render(&self) -> String {
        self.text.clone()
    }
}

#[cfg(test)]
#[path = "contextual_user_fragment.test.rs"]
mod tests;
