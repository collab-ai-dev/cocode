use vercel_ai_provider::AISdkError;
use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::LanguageModelV4Prompt;
use vercel_ai_provider::UserContentPart;

/// Result of converting a prompt to the legacy Completions API format.
pub struct CompletionPrompt {
    /// The formatted prompt text with role prefixes.
    pub prompt: String,
    /// Auto-generated stop sequences (e.g. `["\nuser:"]`).
    pub stop_sequences: Vec<String>,
}

/// Convert a `LanguageModelV4Prompt` to a role-prefixed text prompt for the legacy
/// Completions API.
///
/// Format:
///   - System message (if first) is prepended as plain text
///   - User messages: `"user:\n{content}\n\n"`
///   - Assistant messages: `"assistant:\n{content}\n\n"`
///   - Trailing `"assistant:\n"` to prompt model continuation
///   - Auto-generated stop sequence: `["\nuser:"]`
pub fn convert_to_completion_prompt(
    prompt: &LanguageModelV4Prompt,
) -> Result<CompletionPrompt, AISdkError> {
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
            LanguageModelV4Message::System { content, .. } => {
                return Err(AISdkError::new(format!(
                    "Unexpected system message in prompt: {}",
                    collapse_text_parts(content)?
                )));
            }
            LanguageModelV4Message::Developer { .. } => {
                return Err(AISdkError::new(
                    "Unexpected developer message in prompt".to_string(),
                ));
            }
            LanguageModelV4Message::User { content, .. } => {
                let user_text: String = content
                    .iter()
                    .filter_map(|part| match part {
                        UserContentPart::Text(tp) => Some(tp.text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                text.push_str(&format!("{user}:\n{user_text}\n\n"));
            }
            LanguageModelV4Message::Assistant { content, .. } => {
                let has_tool_call = content
                    .iter()
                    .any(|p| matches!(p, AssistantContentPart::ToolCall(_)));
                if has_tool_call {
                    return Err(AISdkError::new(
                        "Tool-call messages are not supported in the completion API".to_string(),
                    ));
                }
                let assistant_text: String = content
                    .iter()
                    .filter_map(|part| match part {
                        AssistantContentPart::Text(tp) => Some(tp.text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                text.push_str(&format!("{assistant}:\n{assistant_text}\n\n"));
            }
            LanguageModelV4Message::Tool { .. } => {
                return Err(AISdkError::new(
                    "Tool messages are not supported in the completion API".to_string(),
                ));
            }
        }
    }

    // Append assistant prefix to prompt model continuation.
    text.push_str(&format!("{assistant}:\n"));

    Ok(CompletionPrompt {
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
