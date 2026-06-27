use super::TranscriptEntry;

/// Extract a short text snippet from a transcript entry's message content.
pub(super) fn extract_text_content(entry: &TranscriptEntry) -> String {
    let Some(message) = &entry.message else {
        return String::new();
    };

    // Message has a "content" field that is either a string or an array.
    let Some(content) = message.get("content") else {
        return String::new();
    };

    if let Some(text) = content.as_str() {
        return truncate_prompt(text);
    }

    // Array content: find the first text block.
    if let Some(arr) = content.as_array() {
        for block in arr {
            if block.get("type").and_then(|t| t.as_str()) == Some("text")
                && let Some(text) = block.get("text").and_then(|t| t.as_str())
            {
                return truncate_prompt(text);
            }
        }
    }

    String::new()
}

/// Returns true if the candidate text should be skipped when picking
/// the resume-picker's "first prompt" preview.
pub(super) fn is_synthetic_first_prompt_candidate(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed == coco_messages::INTERRUPT_MESSAGE
        || trimmed == coco_messages::INTERRUPT_MESSAGE_FOR_TOOL_USE
        || trimmed.starts_with("[Request interrupted by user")
}

/// Truncate a prompt string for display (200-char limit).
pub(super) fn truncate_prompt(text: &str) -> String {
    let flat = text.replace('\n', " ");
    let trimmed = flat.trim();
    if trimmed.len() > 200 {
        format!("{}...", &trimmed[..200].trim())
    } else {
        trimmed.to_string()
    }
}
