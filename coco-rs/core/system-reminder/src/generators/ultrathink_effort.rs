//! `ultrathink_effort` generator.
//!
//! Fires when the user prompt contains the `ultrathink` keyword, asking
//! the model to apply high reasoning effort for the current turn.
//!
//! Gate chain (all must pass):
//!
//! 1. `ctx.config.attachments.ultrathink_effort` — user opt-in (default off).
//! 2. `ctx.user_input` contains the word `ultrathink` (case-insensitive,
//!    word-boundary — see [`contains_ultrathink_keyword`]).

use async_trait::async_trait;

use crate::error::Result;
use crate::generator::AttachmentGenerator;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use crate::types::SystemReminder;
use coco_config::SystemReminderConfig;

/// Reminder body — level is always `"high"`, so it is hardcoded.
const BODY: &str =
    "The user has requested reasoning effort level: high. Apply this to the current turn.";

const KEYWORD: &str = "ultrathink";

/// True if `text` contains the `ultrathink` keyword as a whole word
/// (case-insensitive, word-boundary). Hand-rolled over `char_indices`
/// so there is no fallible regex construction on the hot path.
///
/// Word boundary: delimits between a word character (`[A-Za-z0-9_]`)
/// and a non-word character — see [`is_word_char`].
pub fn contains_ultrathink_keyword(text: &str) -> bool {
    let kw = KEYWORD;
    let kw_len = kw.len();
    let bytes = text.as_bytes();
    if bytes.len() < kw_len {
        return false;
    }
    for (i, _) in text.char_indices() {
        let end = i + kw_len;
        if end > bytes.len() {
            return false;
        }
        let candidate = &bytes[i..end];
        if !candidate.eq_ignore_ascii_case(kw.as_bytes()) {
            continue;
        }
        let before_ok = i == 0
            || text[..i]
                .chars()
                .next_back()
                .is_some_and(|c| !is_word_char(c));
        let after_ok =
            end == bytes.len() || text[end..].chars().next().is_some_and(|c| !is_word_char(c));
        if before_ok && after_ok {
            return true;
        }
    }
    false
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Nudge the model to apply high reasoning effort when the user said
/// `ultrathink`.
#[derive(Debug, Default)]
pub struct UltrathinkEffortGenerator;

#[async_trait]
impl AttachmentGenerator for UltrathinkEffortGenerator {
    fn name(&self) -> &str {
        "UltrathinkEffortGenerator"
    }

    fn attachment_type(&self) -> AttachmentType {
        AttachmentType::UltrathinkEffort
    }

    fn is_enabled(&self, config: &SystemReminderConfig) -> bool {
        config.attachments.ultrathink_effort
    }

    async fn generate(&self, ctx: &GeneratorContext<'_>) -> Result<Option<SystemReminder>> {
        let Some(input) = ctx.user_input.as_deref() else {
            return Ok(None);
        };
        if !contains_ultrathink_keyword(input) {
            return Ok(None);
        }
        Ok(Some(SystemReminder::new(
            AttachmentType::UltrathinkEffort,
            BODY,
        )))
    }
}

#[cfg(test)]
#[path = "ultrathink_effort.test.rs"]
mod tests;
