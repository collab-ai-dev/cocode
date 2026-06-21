//! Prompt and message types for language models.

use serde::Deserialize;
use serde::Serialize;

use crate::content::AssistantContentPart;
use crate::content::ToolContentPart;
use crate::content::UserContentPart;
use crate::shared::ProviderOptions;

/// A prompt for a language model (V4).
///
/// This is a vector of messages that form the conversation history.
pub type LanguageModelV4Prompt = Vec<LanguageModelV4Message>;

/// A message in a language model prompt.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum LanguageModelV4Message {
    /// A system message.
    System {
        /// The system message content parts.
        content: Vec<UserContentPart>,
        /// Provider-specific options.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// A developer message.
    Developer {
        /// The developer message content parts.
        content: Vec<UserContentPart>,
        /// Provider-specific options.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// A user message.
    User {
        /// The user message content parts.
        content: Vec<UserContentPart>,
        /// Provider-specific options.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// An assistant message.
    Assistant {
        /// The assistant message content parts.
        content: Vec<AssistantContentPart>,
        /// Provider-specific options.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// A tool message.
    Tool {
        /// The tool message content parts.
        content: Vec<ToolContentPart>,
        /// Provider-specific options.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
}

impl LanguageModelV4Message {
    /// Create a system message.
    pub fn system(content: impl Into<String>) -> Self {
        Self::system_parts(vec![UserContentPart::text(content)])
    }

    /// Create a system message with content parts.
    pub fn system_parts(parts: Vec<UserContentPart>) -> Self {
        Self::System {
            content: parts,
            provider_options: None,
        }
    }

    /// Create a system message with provider options.
    pub fn system_with_options(content: impl Into<String>, options: ProviderOptions) -> Self {
        Self::system_parts_with_options(vec![UserContentPart::text(content)], options)
    }

    /// Create a system message with content parts and provider options.
    pub fn system_parts_with_options(
        parts: Vec<UserContentPart>,
        options: ProviderOptions,
    ) -> Self {
        Self::System {
            content: parts,
            provider_options: Some(options),
        }
    }

    /// Create a developer message with text.
    pub fn developer_text(content: impl Into<String>) -> Self {
        Self::developer(vec![UserContentPart::text(content)])
    }

    /// Create a developer message with content parts.
    pub fn developer(parts: Vec<UserContentPart>) -> Self {
        Self::Developer {
            content: parts,
            provider_options: None,
        }
    }

    /// Create a developer message with content parts and provider options.
    pub fn developer_with_options(parts: Vec<UserContentPart>, options: ProviderOptions) -> Self {
        Self::Developer {
            content: parts,
            provider_options: Some(options),
        }
    }

    /// Create a user message with text.
    pub fn user_text(content: impl Into<String>) -> Self {
        Self::User {
            content: vec![UserContentPart::text(content)],
            provider_options: None,
        }
    }

    /// Create a user message with content parts.
    pub fn user(parts: Vec<UserContentPart>) -> Self {
        Self::User {
            content: parts,
            provider_options: None,
        }
    }

    /// Create a user message with content parts and provider options.
    pub fn user_with_options(parts: Vec<UserContentPart>, options: ProviderOptions) -> Self {
        Self::User {
            content: parts,
            provider_options: Some(options),
        }
    }

    /// Create an assistant message with text.
    pub fn assistant_text(content: impl Into<String>) -> Self {
        Self::Assistant {
            content: vec![AssistantContentPart::text(content)],
            provider_options: None,
        }
    }

    /// Create an assistant message with content parts.
    pub fn assistant(parts: Vec<AssistantContentPart>) -> Self {
        Self::Assistant {
            content: parts,
            provider_options: None,
        }
    }

    /// Create an assistant message with content parts and provider options.
    pub fn assistant_with_options(
        parts: Vec<AssistantContentPart>,
        options: ProviderOptions,
    ) -> Self {
        Self::Assistant {
            content: parts,
            provider_options: Some(options),
        }
    }

    /// Create a tool message.
    pub fn tool(parts: Vec<ToolContentPart>) -> Self {
        Self::Tool {
            content: parts,
            provider_options: None,
        }
    }

    /// Create a tool message with provider options.
    pub fn tool_with_options(parts: Vec<ToolContentPart>, options: ProviderOptions) -> Self {
        Self::Tool {
            content: parts,
            provider_options: Some(options),
        }
    }

    /// Check if this is a system message.
    pub fn is_system(&self) -> bool {
        matches!(self, Self::System { .. })
    }

    /// Check if this is a developer message.
    pub fn is_developer(&self) -> bool {
        matches!(self, Self::Developer { .. })
    }

    /// Check if this is a user message.
    pub fn is_user(&self) -> bool {
        matches!(self, Self::User { .. })
    }

    /// Check if this is an assistant message.
    pub fn is_assistant(&self) -> bool {
        matches!(self, Self::Assistant { .. })
    }

    /// Check if this is a tool message.
    pub fn is_tool(&self) -> bool {
        matches!(self, Self::Tool { .. })
    }
}

/// Builder for creating prompts.
#[derive(Debug, Default)]
pub struct PromptBuilder {
    messages: LanguageModelV4Prompt,
}

impl PromptBuilder {
    /// Create a new prompt builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a system message.
    pub fn system(mut self, content: impl Into<String>) -> Self {
        self.messages.push(LanguageModelV4Message::system(content));
        self
    }

    /// Add a developer message.
    pub fn developer(mut self, content: impl Into<String>) -> Self {
        self.messages
            .push(LanguageModelV4Message::developer_text(content));
        self
    }

    /// Add a user message with text.
    pub fn user(mut self, content: impl Into<String>) -> Self {
        self.messages
            .push(LanguageModelV4Message::user_text(content));
        self
    }

    /// Add a user message with content parts.
    pub fn user_parts(mut self, parts: Vec<UserContentPart>) -> Self {
        self.messages.push(LanguageModelV4Message::user(parts));
        self
    }

    /// Add an assistant message with text.
    pub fn assistant(mut self, content: impl Into<String>) -> Self {
        self.messages
            .push(LanguageModelV4Message::assistant_text(content));
        self
    }

    /// Add an assistant message with content parts.
    pub fn assistant_parts(mut self, parts: Vec<AssistantContentPart>) -> Self {
        self.messages.push(LanguageModelV4Message::assistant(parts));
        self
    }

    /// Build the prompt.
    pub fn build(self) -> LanguageModelV4Prompt {
        self.messages
    }
}

#[cfg(test)]
#[path = "prompt.test.rs"]
mod tests;
