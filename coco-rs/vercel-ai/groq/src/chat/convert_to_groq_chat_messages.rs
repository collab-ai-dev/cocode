use base64::Engine as _;
use serde_json::Value;
use serde_json::json;
use vercel_ai_provider::AISdkError;
use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::FileRawData;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::LanguageModelV4Prompt;
use vercel_ai_provider::SharedV4FileData;
use vercel_ai_provider::ToolContentPart;
use vercel_ai_provider::ToolResultContent;
use vercel_ai_provider::ToolResultContentPart;
use vercel_ai_provider::UserContentPart;
use vercel_ai_provider::Warning;

/// Convert a `LanguageModelV4Prompt` into Groq Chat Completions messages.
///
/// Mirrors `convert-to-groq-chat-messages.ts`. Returns `Ok((messages, warnings))`.
/// Only image file parts are supported for user content; any other file part
/// yields an unsupported-functionality error (matching the TS thrower).
pub fn convert_to_groq_chat_messages(
    prompt: &LanguageModelV4Prompt,
) -> Result<(Vec<Value>, Vec<Warning>), AISdkError> {
    let mut messages: Vec<Value> = Vec::new();
    let mut warnings: Vec<Warning> = Vec::new();

    for msg in prompt {
        match msg {
            // Groq/Llama models expose only system/user/assistant/tool roles,
            // so developer messages collapse into a system message.
            LanguageModelV4Message::System { content, .. }
            | LanguageModelV4Message::Developer { content, .. } => {
                let text = collapse_text_parts(content, &mut warnings, "system message");
                messages.push(json!({ "role": "system", "content": text }));
            }

            LanguageModelV4Message::User { content, .. } => {
                // A lone text part collapses to a plain string.
                if content.len() == 1
                    && let Some(UserContentPart::Text(text_part)) = content.first()
                {
                    messages.push(json!({ "role": "user", "content": text_part.text }));
                    continue;
                }

                let parts = content
                    .iter()
                    .map(convert_user_part)
                    .collect::<Result<Vec<Value>, AISdkError>>()?;
                messages.push(json!({ "role": "user", "content": parts }));
            }

            LanguageModelV4Message::Assistant { content, .. } => {
                let mut text = String::new();
                let mut reasoning = String::new();
                let mut tool_calls: Vec<Value> = Vec::new();

                for part in content {
                    match part {
                        // Groq supports reasoning for tool-calls in multi-turn
                        // conversations (vercel/ai#7860).
                        AssistantContentPart::Reasoning(r) => reasoning.push_str(&r.text),
                        AssistantContentPart::Text(t) => text.push_str(&t.text),
                        AssistantContentPart::ToolCall(tc) => {
                            let arguments = serde_json::to_string(&tc.input).unwrap_or_default();
                            tool_calls.push(json!({
                                "id": tc.tool_call_id,
                                "type": "function",
                                "function": { "name": tc.tool_name, "arguments": arguments },
                            }));
                        }
                        // Other assistant parts have no Groq wire representation
                        // and are dropped.
                        AssistantContentPart::File(_)
                        | AssistantContentPart::ReasoningFile(_)
                        | AssistantContentPart::Custom(_)
                        | AssistantContentPart::ToolResult(_)
                        | AssistantContentPart::Source(_)
                        | AssistantContentPart::ToolApprovalRequest(_) => {}
                    }
                }

                let mut assistant = json!({ "role": "assistant", "content": text });
                if !reasoning.is_empty() {
                    assistant["reasoning"] = Value::String(reasoning);
                }
                if !tool_calls.is_empty() {
                    assistant["tool_calls"] = Value::Array(tool_calls);
                }
                messages.push(assistant);
            }

            LanguageModelV4Message::Tool { content, .. } => {
                for part in content {
                    match part {
                        ToolContentPart::ToolResult(result) => {
                            let content_value = serialize_tool_result_output(&result.output);
                            messages.push(json!({
                                "role": "tool",
                                "tool_call_id": result.tool_call_id,
                                "content": content_value,
                            }));
                        }
                        // Approval responses have no Chat API representation.
                        ToolContentPart::ToolApprovalResponse(_) => {}
                    }
                }
            }
        }
    }

    Ok((messages, warnings))
}

/// Collapse content parts into a single string, warning on dropped non-text.
fn collapse_text_parts(
    parts: &[UserContentPart],
    warnings: &mut Vec<Warning>,
    context: &str,
) -> String {
    let mut text = String::new();
    for part in parts {
        match part {
            UserContentPart::Text(t) => text.push_str(&t.text),
            UserContentPart::File(_) => warnings.push(Warning::unsupported_with_details(
                "non-text prompt part",
                format!("{context} contains a non-text part that was dropped"),
            )),
        }
    }
    text
}

/// Convert a single user content part to Groq's wire shape.
fn convert_user_part(part: &UserContentPart) -> Result<Value, AISdkError> {
    match part {
        UserContentPart::Text(t) => Ok(json!({ "type": "text", "text": t.text })),
        UserContentPart::File(file) => {
            if !file.media_type.starts_with("image/") {
                return Err(AISdkError::new(
                    "Unsupported functionality: Non-image file content parts",
                ));
            }
            let url = image_url(&file.data, &file.media_type)?;
            Ok(json!({ "type": "image_url", "image_url": { "url": url } }))
        }
    }
}

/// Build an image URL (direct URL or `data:` URI) for a file part.
fn image_url(data: &SharedV4FileData, media_type: &str) -> Result<String, AISdkError> {
    // "image/*" is not a full media type; fall back to a concrete default.
    let full_type = if media_type == "image/*" {
        "image/jpeg"
    } else {
        media_type
    };
    match data {
        SharedV4FileData::Url { url } => Ok(url.clone()),
        SharedV4FileData::Data { data } => {
            let b64 = file_raw_data_to_base64(data);
            Ok(format!("data:{full_type};base64,{b64}"))
        }
        SharedV4FileData::Text { .. } => Err(AISdkError::new(
            "Unsupported functionality: text file parts",
        )),
        SharedV4FileData::Reference { .. } => Err(AISdkError::new(
            "Unsupported functionality: file parts with provider references",
        )),
    }
}

fn file_raw_data_to_base64(raw: &FileRawData) -> String {
    match raw {
        FileRawData::Base64(b64) => b64.clone(),
        FileRawData::Bytes(bytes) => base64::engine::general_purpose::STANDARD.encode(bytes),
    }
}

/// Stringify a tool result output for the `tool` message `content` field.
///
/// Mirrors the `switch (output.type)` in `convert-to-groq-chat-messages.ts`.
fn serialize_tool_result_output(output: &ToolResultContent) -> String {
    match output {
        ToolResultContent::Text { value, .. } | ToolResultContent::ErrorText { value, .. } => {
            value.clone()
        }
        ToolResultContent::ExecutionDenied { reason, .. } => reason
            .clone()
            .unwrap_or_else(|| "Tool call execution denied.".to_string()),
        ToolResultContent::Json { value, .. } | ToolResultContent::ErrorJson { value, .. } => {
            serde_json::to_string(value).unwrap_or_default()
        }
        // Groq's `tool` role carries a single string and cannot hold image or
        // document blocks. Text parts pass through; non-text parts degrade to a
        // visible marker instead of dumping raw base64 into the prompt (mirrors
        // the openai / openai-compatible siblings).
        ToolResultContent::Content { value, .. } => value
            .iter()
            .map(|part| match part {
                ToolResultContentPart::Text { text, .. } => text.clone(),
                ToolResultContentPart::FileData { media_type, .. }
                | ToolResultContentPart::FileUrl { media_type, .. } => format!(
                    "[{media_type} content omitted — provider doesn't support multimodal tool results]"
                ),
                ToolResultContentPart::FileReference { .. } => {
                    "[file reference omitted — provider doesn't support multimodal tool results]"
                        .to_string()
                }
                ToolResultContentPart::Custom { .. } => {
                    "[custom provider-specific content omitted]".to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

#[cfg(test)]
#[path = "convert_to_groq_chat_messages.test.rs"]
mod tests;
