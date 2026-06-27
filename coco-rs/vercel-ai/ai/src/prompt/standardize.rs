//! Standardize prompt format.
//!
//! This module provides functions for converting various prompt formats
//! into a standardized format that can be processed uniformly.

use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::LanguageModelV4Prompt;

use crate::error::AIError;

/// A standardized prompt with system messages and conversation messages.
#[derive(Debug, Clone, Default)]
pub struct StandardizedPrompt {
    /// System messages (prepended to the conversation).
    pub system: Option<Vec<LanguageModelV4Message>>,
    /// The conversation messages.
    pub messages: LanguageModelV4Prompt,
}

impl StandardizedPrompt {
    /// Create a new empty standardized prompt.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add system messages.
    pub fn with_system(mut self, system: Vec<LanguageModelV4Message>) -> Self {
        self.system = Some(system);
        self
    }

    /// Add a single system message.
    pub fn with_system_text(mut self, text: impl Into<String>) -> Self {
        let system = vec![LanguageModelV4Message::system(text)];
        self.system = Some(system);
        self
    }

    /// Set the messages.
    pub fn with_messages(mut self, messages: LanguageModelV4Prompt) -> Self {
        self.messages = messages;
        self
    }
}

/// Standardize a prompt from text.
///
/// Converts a simple text prompt into a standardized format with a user message.
pub fn standardize_text_prompt(text: impl Into<String>) -> StandardizedPrompt {
    StandardizedPrompt::new().with_messages(vec![LanguageModelV4Message::user_text(text)])
}

/// Standardize a prompt from messages.
///
/// Validates and standardizes a list of messages.
pub fn standardize_messages_prompt(
    messages: LanguageModelV4Prompt,
) -> Result<StandardizedPrompt, AIError> {
    if messages.is_empty() {
        return Err(AIError::InvalidArgument(
            "messages must not be empty".to_string(),
        ));
    }

    Ok(StandardizedPrompt::new().with_messages(messages))
}

/// Standardize a prompt with optional system message.
///
/// This is the main entry point for standardizing prompts.
///
/// **Security**: as of vercel/ai #14752, system messages embedded in
/// `messages` are rejected by default — they create a prompt-injection
/// vector when the messages array comes from untrusted input (web UI,
/// tool output, etc.). Use [`standardize_prompt_with_options`] +
/// `allow_system_in_messages: true` to opt out for legacy callers that
/// genuinely need interleaved system messages.
pub fn standardize_prompt(
    prompt: Option<String>,
    messages: Option<LanguageModelV4Prompt>,
    system: Option<String>,
) -> Result<StandardizedPrompt, AIError> {
    standardize_prompt_with_options(prompt, messages, system, false)
}

/// Like [`standardize_prompt`], but allows the caller to opt into
/// permitting system messages inside the `messages` array.
///
/// `allow_system_in_messages = true` matches legacy v4 behavior; the
/// new default (`false`) matches `@ai-sdk/ai` >= post-#14752.
pub fn standardize_prompt_with_options(
    prompt: Option<String>,
    messages: Option<LanguageModelV4Prompt>,
    system: Option<String>,
    allow_system_in_messages: bool,
) -> Result<StandardizedPrompt, AIError> {
    // Validate that either prompt or messages is provided.
    let (text_prompt, msgs) = match (prompt, messages) {
        (Some(_), Some(_)) => {
            return Err(AIError::InvalidArgument(
                "prompt and messages cannot be defined at the same time".to_string(),
            ));
        }
        (None, None) => {
            return Err(AIError::InvalidArgument(
                "prompt or messages must be defined".to_string(),
            ));
        }
        (Some(p), None) => (Some(p), None),
        (None, Some(m)) => (None, Some(m)),
    };

    let standardized = match (text_prompt, msgs) {
        (Some(p), None) => standardize_text_prompt(p),
        (None, Some(msgs)) => {
            // Reject embedded system messages unless explicitly opted in.
            if !allow_system_in_messages
                && msgs
                    .iter()
                    .any(|m| matches!(m, LanguageModelV4Message::System { .. }))
            {
                return Err(AIError::InvalidArgument(
                    "System messages are not allowed in the prompt or messages fields. \
                     Use the system option instead, or set allow_system_in_messages=true \
                     if interleaved system messages are intentional."
                        .to_string(),
                ));
            }
            standardize_messages_prompt(msgs)?
        }
        _ => unreachable!("guarded above"),
    };

    Ok(if let Some(sys) = system {
        standardized.with_system_text(sys)
    } else {
        standardized
    })
}

#[cfg(test)]
#[path = "standardize.test.rs"]
mod tests;
