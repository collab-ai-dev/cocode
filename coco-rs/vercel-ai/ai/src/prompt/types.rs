//! Prompt types for constructing prompts.
//!
//! This module provides the `Prompt` type which is the main way to specify
//! input to the model in the high-level API.

use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::LanguageModelV4Prompt;

/// A prompt for a language model.
///
/// This can be either a simple text prompt or a conversation with multiple messages.
#[derive(Debug, Clone, Default)]
pub struct Prompt {
    /// System prompt (prepended to the conversation).
    pub system: Option<SystemPrompt>,
    /// The main content of the prompt.
    pub content: PromptContent,
}

/// System prompt configuration.
#[derive(Debug, Clone)]
pub enum SystemPrompt {
    /// A simple text system prompt.
    Text(String),
}

impl Prompt {
    /// Create a new empty prompt.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a prompt from a simple text string.
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            system: None,
            content: PromptContent::Text(text.into()),
        }
    }

    /// Create a prompt from a conversation (list of messages).
    pub fn messages(messages: Vec<LanguageModelV4Message>) -> Self {
        Self {
            system: None,
            content: PromptContent::Messages(messages),
        }
    }

    /// Add a system prompt.
    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = Some(SystemPrompt::Text(system.into()));
        self
    }

    /// Add a system prompt from a SystemPrompt enum.
    pub fn with_system_prompt(mut self, system: SystemPrompt) -> Self {
        self.system = Some(system);
        self
    }

    /// Convert to a LanguageModelV4Prompt.
    pub fn to_model_prompt(&self) -> LanguageModelV4Prompt {
        let mut messages = Vec::new();

        // Add system message if present
        if let Some(SystemPrompt::Text(text)) = &self.system {
            messages.push(LanguageModelV4Message::system(text));
        }

        // Add content
        match &self.content {
            PromptContent::Text(text) => {
                messages.push(LanguageModelV4Message::user_text(text));
            }
            PromptContent::Messages(msgs) => {
                // If there's already a system message in the messages, don't add it again
                let has_system = msgs.iter().any(LanguageModelV4Message::is_system);
                if !has_system {
                    messages.extend(msgs.clone());
                } else {
                    // Messages already include system, use them directly
                    return msgs.clone();
                }
            }
        }

        messages
    }
}

impl From<String> for Prompt {
    fn from(text: String) -> Self {
        Self::user(text)
    }
}

impl From<&str> for Prompt {
    fn from(text: &str) -> Self {
        Self::user(text)
    }
}

impl From<Vec<LanguageModelV4Message>> for Prompt {
    fn from(messages: Vec<LanguageModelV4Message>) -> Self {
        Self::messages(messages)
    }
}

/// Content of a prompt.
#[derive(Debug, Clone)]
pub enum PromptContent {
    /// A simple text prompt.
    Text(String),
    /// A conversation with multiple messages.
    Messages(Vec<LanguageModelV4Message>),
}

impl Default for PromptContent {
    fn default() -> Self {
        Self::Text(String::new())
    }
}

#[cfg(test)]
#[path = "types.test.rs"]
mod tests;
