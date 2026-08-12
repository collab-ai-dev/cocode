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

/// Aggregate skill-listing reminder with one context-owned budget. Producers
/// may impose smaller semantic budgets, but prompt assembly gets a final hard
/// stop here so per-entry caps can never add up to unbounded context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillListingFragment {
    text: String,
}

impl SkillListingFragment {
    /// Body budget. The reminder injector adds a 39-byte XML envelope, so the
    /// body reserves enough space to keep the fully rendered item within the
    /// public 4 KB / 1K-token contract.
    pub const MAX_BYTES: usize = 3_960;
    pub const MAX_TOKENS: i64 = 990;
    pub const MAX_RENDERED_BYTES: usize = 4_000;
    pub const MAX_RENDERED_TOKENS: i64 = 1_000;

    pub fn new(text: &str) -> Self {
        let mut bounded =
            coco_utils_string::take_bytes_at_char_boundary(text, Self::MAX_BYTES).to_string();
        while estimate_text_tokens(&bounded) > Self::MAX_TOKENS {
            bounded.pop();
        }
        Self { text: bounded }
    }
}

impl ContextualUserFragment for SkillListingFragment {
    fn render(&self) -> String {
        self.text.clone()
    }
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
