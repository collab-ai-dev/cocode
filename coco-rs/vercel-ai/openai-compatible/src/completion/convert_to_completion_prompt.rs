use vercel_ai_provider::AISdkError;
use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::LanguageModelV4Prompt;
use vercel_ai_provider::UserContentPart;

/// Result of converting a prompt to completion format.
pub struct CompletionPromptResult {
    /// The formatted prompt text.
    pub prompt: String,
    /// Stop sequences to use (e.g., `["\nuser:"]`).
    pub stop_sequences: Vec<String>,
}

/// Convert a `LanguageModelV4Prompt` to a single text prompt for the legacy Completions API.
///
/// Produces structured format matching the TS implementation:
/// ```text
/// {system}\n\n
/// user:\n{content}\n\n
/// assistant:\n{content}\n\n
/// assistant:\n
/// ```
///
/// Returns the prompt text and stop sequences (`["\nuser:"]`).
///
/// Errors on:
/// - System messages after the first
/// - Tool messages (unsupported in completions)
/// - Non-text content in user messages
pub fn convert_to_completion_prompt(
    prompt: &LanguageModelV4Prompt,
) -> Result<CompletionPromptResult, AISdkError> {
    let user = "user";
    let assistant = "assistant";

    let mut text = String::new();
    let mut iter = prompt.iter().peekable();

    // Leading system/developer messages are prepended as plain text.
    while matches!(
        iter.peek(),
        Some(LanguageModelV4Message::System { .. } | LanguageModelV4Message::Developer { .. })
    ) {
        let Some(msg) = iter.next() else {
            break;
        };
        let content = match msg {
            LanguageModelV4Message::System { content, .. }
            | LanguageModelV4Message::Developer { content, .. } => content,
            _ => unreachable!(),
        };
        text.push_str(&collapse_text_parts(content)?);
        text.push_str("\n\n");
    }

    for msg in iter {
        match msg {
            LanguageModelV4Message::System { .. } => {
                return Err(AISdkError::new(
                    "Invalid prompt: system messages are only supported as the first message in completion prompts",
                ));
            }
            LanguageModelV4Message::Developer { .. } => {
                return Err(AISdkError::new(
                    "Invalid prompt: developer messages are only supported before conversation messages in completion prompts",
                ));
            }
            LanguageModelV4Message::User { content, .. } => {
                let user_message: String = content
                    .iter()
                    .filter_map(|part| {
                        if let UserContentPart::Text(tp) = part {
                            Some(tp.text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("");
                text.push_str(&format!("{user}:\n{user_message}\n\n"));
            }
            LanguageModelV4Message::Assistant { content, .. } => {
                // Check for unsupported tool-call parts
                if content
                    .iter()
                    .any(|p| matches!(p, AssistantContentPart::ToolCall(_)))
                {
                    return Err(AISdkError::new(
                        "Unsupported functionality: tool-call messages in completion prompts",
                    ));
                }

                let assistant_message: String = content
                    .iter()
                    .filter_map(|part| {
                        if let AssistantContentPart::Text(tp) = part {
                            Some(tp.text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("");
                text.push_str(&format!("{assistant}:\n{assistant_message}\n\n"));
            }
            LanguageModelV4Message::Tool { .. } => {
                return Err(AISdkError::new(
                    "Unsupported functionality: tool messages are not supported in completion prompts",
                ));
            }
        }
    }

    // Add assistant prefix for the model to continue from
    text.push_str(&format!("{assistant}:\n"));

    Ok(CompletionPromptResult {
        prompt: text,
        stop_sequences: vec![format!("\n{user}:")],
    })
}

fn collapse_text_parts(parts: &[UserContentPart]) -> Result<String, AISdkError> {
    let mut text = String::new();
    for part in parts {
        match part {
            UserContentPart::Text(text_part) => text.push_str(&text_part.text),
            UserContentPart::File(_) => {
                return Err(AISdkError::new(
                    "Non-text content is not supported in completion system/developer messages",
                ));
            }
        }
    }
    Ok(text)
}

#[cfg(test)]
#[path = "convert_to_completion_prompt.test.rs"]
mod tests;
