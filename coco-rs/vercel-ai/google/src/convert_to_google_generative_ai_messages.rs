//! Convert Vercel AI SDK messages to Google Generative AI format.

use base64::Engine as _;
use serde_json::Value;
use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::FileRawData;
use vercel_ai_provider::LanguageModelV4FileData;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::SharedV4FileData;
use vercel_ai_provider::ToolContentPart;
use vercel_ai_provider::ToolResultContent;
use vercel_ai_provider::ToolResultContentPart;
use vercel_ai_provider::UserContentPart;
use vercel_ai_provider::Warning;

use crate::google_generative_ai_prompt::FileDataPart;
use crate::google_generative_ai_prompt::FunctionCallPart;
use crate::google_generative_ai_prompt::FunctionResponsePart;
use crate::google_generative_ai_prompt::GoogleContentRole;
use crate::google_generative_ai_prompt::GoogleGenerativeAIContent;
use crate::google_generative_ai_prompt::GoogleGenerativeAIContentPart;
use crate::google_generative_ai_prompt::GoogleGenerativeAIPrompt;
use crate::google_generative_ai_prompt::GoogleGenerativeAISystemInstruction;
use crate::google_generative_ai_prompt::GoogleTextPart;
use crate::google_generative_ai_prompt::InlineDataPart;

/// Options for message conversion.
pub struct ConvertOptions {
    /// Whether the model supports system instructions (Gemma models don't).
    pub supports_system_instruction: bool,
    /// Provider options namespace ("google" or "vertex") for thoughtSignature extraction.
    pub provider_options_name: String,
    /// Whether the model supports multimodal parts inside functionResponse (Gemini 3+).
    pub supports_function_response_parts: bool,
}

impl Default for ConvertOptions {
    fn default() -> Self {
        Self {
            supports_system_instruction: true,
            provider_options_name: "google".to_string(),
            supports_function_response_parts: true,
        }
    }
}

/// Convert Vercel AI SDK messages to Google Generative AI prompt format.
///
/// Returns `Err` if system messages appear after non-system messages.
pub fn convert_to_google_generative_ai_messages(
    prompt: &[LanguageModelV4Message],
    options: &ConvertOptions,
) -> Result<GoogleGenerativeAIPrompt, String> {
    let (prompt, _) = convert_to_google_generative_ai_messages_with_warnings(prompt, options)?;
    Ok(prompt)
}

pub fn convert_to_google_generative_ai_messages_with_warnings(
    prompt: &[LanguageModelV4Message],
    options: &ConvertOptions,
) -> Result<(GoogleGenerativeAIPrompt, Vec<Warning>), String> {
    let mut system_instruction_parts: Vec<GoogleTextPart> = Vec::new();
    let mut contents: Vec<GoogleGenerativeAIContent> = Vec::new();
    let mut system_messages_allowed = true;
    let mut warnings = Vec::new();

    for message in prompt {
        match message {
            LanguageModelV4Message::System { content, .. } => {
                if !system_messages_allowed {
                    return Err(
                        "system messages are only supported at the beginning of the conversation"
                            .to_string(),
                    );
                }
                let text = collapse_text_parts(content, &mut warnings, "system message");
                system_instruction_parts.push(GoogleTextPart { text });
            }
            LanguageModelV4Message::Developer { content, .. } => {
                if !system_messages_allowed {
                    return Err(
                        "developer messages are only supported at the beginning of the conversation"
                            .to_string(),
                    );
                }
                let text = collapse_text_parts(content, &mut warnings, "developer message");
                system_instruction_parts.push(GoogleTextPart { text });
            }
            LanguageModelV4Message::User { content, .. } => {
                system_messages_allowed = false;
                let parts = convert_user_content_parts(content);
                if !parts.is_empty() {
                    contents.push(GoogleGenerativeAIContent {
                        role: GoogleContentRole::User,
                        parts,
                    });
                }
            }
            LanguageModelV4Message::Assistant { content, .. } => {
                system_messages_allowed = false;
                let parts =
                    convert_assistant_content_parts(content, &options.provider_options_name)?;
                if !parts.is_empty() {
                    contents.push(GoogleGenerativeAIContent {
                        role: GoogleContentRole::Model,
                        parts,
                    });
                }
            }
            LanguageModelV4Message::Tool { content, .. } => {
                system_messages_allowed = false;
                let parts =
                    convert_tool_content_parts(content, options.supports_function_response_parts);
                if !parts.is_empty() {
                    contents.push(GoogleGenerativeAIContent {
                        role: GoogleContentRole::User,
                        parts,
                    });
                }
            }
        }
    }

    // For Gemma models: prepend system text to first user message
    if !options.supports_system_instruction
        && !system_instruction_parts.is_empty()
        && !contents.is_empty()
        && contents[0].role == GoogleContentRole::User
    {
        let system_text = system_instruction_parts
            .iter()
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        contents[0].parts.insert(
            0,
            GoogleGenerativeAIContentPart::Text {
                text: format!("{system_text}\n\n"),
                thought: None,
                thought_signature: None,
            },
        );
    }

    // Only emit systemInstruction when the model supports it
    let system_instruction =
        if system_instruction_parts.is_empty() || !options.supports_system_instruction {
            None
        } else {
            Some(GoogleGenerativeAISystemInstruction {
                parts: system_instruction_parts,
            })
        };

    Ok((
        GoogleGenerativeAIPrompt {
            system_instruction,
            contents,
        },
        warnings,
    ))
}

fn collapse_text_parts(
    parts: &[UserContentPart],
    warnings: &mut Vec<Warning>,
    context: &str,
) -> String {
    let mut text = String::new();
    for part in parts {
        match part {
            UserContentPart::Text(text_part) => text.push_str(&text_part.text),
            UserContentPart::File(_) => warnings.push(Warning::unsupported_with_details(
                "non-text prompt part",
                format!("{context} contains a non-text part that was dropped"),
            )),
        }
    }
    text
}

fn convert_user_content_parts(parts: &[UserContentPart]) -> Vec<GoogleGenerativeAIContentPart> {
    let mut result = Vec::new();
    for part in parts {
        match part {
            UserContentPart::Text(text_part) => {
                result.push(GoogleGenerativeAIContentPart::Text {
                    text: text_part.text.clone(),
                    thought: None,
                    thought_signature: None,
                });
            }
            UserContentPart::File(file_part) => {
                result.push(convert_file_part(&file_part.data, &file_part.media_type));
            }
        }
    }
    result
}

/// Extract thoughtSignature from a part's provider metadata.
fn extract_thought_signature(
    provider_metadata: &Option<vercel_ai_provider::ProviderMetadata>,
    provider_options_name: &str,
) -> Option<String> {
    let meta = provider_metadata.as_ref()?;
    // Try provider-specific namespace first, then fallback
    let fallback = if provider_options_name == "vertex" {
        "google"
    } else {
        "vertex"
    };
    let opts = meta
        .get(provider_options_name)
        .or_else(|| meta.get(fallback))?;
    opts.get("thoughtSignature")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn convert_assistant_content_parts(
    parts: &[AssistantContentPart],
    provider_options_name: &str,
) -> Result<Vec<GoogleGenerativeAIContentPart>, String> {
    let mut result = Vec::new();
    for part in parts {
        match part {
            AssistantContentPart::Text(text_part) => {
                if text_part.text.is_empty() {
                    continue;
                }
                let ts =
                    extract_thought_signature(&text_part.provider_metadata, provider_options_name);
                result.push(GoogleGenerativeAIContentPart::Text {
                    text: text_part.text.clone(),
                    thought: None,
                    thought_signature: ts,
                });
            }
            AssistantContentPart::File(file_part) => {
                if matches!(&file_part.data, SharedV4FileData::Url { .. }) {
                    return Err(
                        "File data URLs in assistant messages are not supported".to_string()
                    );
                }
                let ts =
                    extract_thought_signature(&file_part.provider_metadata, provider_options_name);
                let thought = file_part
                    .provider_metadata
                    .as_ref()
                    .and_then(|m| {
                        let key = provider_options_name;
                        let fallback = if key == "vertex" { "google" } else { "vertex" };
                        m.get(key).or_else(|| m.get(fallback))
                    })
                    .and_then(|opts| opts.get("thought"))
                    .and_then(serde_json::Value::as_bool)
                    .filter(|&v| v)
                    .map(|_| true);
                let base_part = convert_file_part_to_inline(&file_part.data, &file_part.media_type);
                match base_part {
                    GoogleGenerativeAIContentPart::InlineData { inline_data, .. } => {
                        result.push(GoogleGenerativeAIContentPart::InlineData {
                            inline_data,
                            thought,
                            thought_signature: ts,
                        });
                    }
                    other => result.push(other),
                }
            }
            AssistantContentPart::ReasoningFile(rf_part) => {
                if matches!(&rf_part.data, LanguageModelV4FileData::Url { .. }) {
                    return Err(
                        "File data URLs in assistant messages are not supported".to_string()
                    );
                }
                let ts =
                    extract_thought_signature(&rf_part.provider_metadata, provider_options_name);
                let base_part = convert_lm_file_data_to_inline(&rf_part.data, &rf_part.media_type);
                match base_part {
                    GoogleGenerativeAIContentPart::InlineData { inline_data, .. } => {
                        result.push(GoogleGenerativeAIContentPart::InlineData {
                            inline_data,
                            thought: Some(true),
                            thought_signature: ts,
                        });
                    }
                    other => result.push(other),
                }
            }
            AssistantContentPart::Reasoning(reasoning_part) => {
                if reasoning_part.text.is_empty() {
                    continue;
                }
                let ts = extract_thought_signature(
                    &reasoning_part.provider_metadata,
                    provider_options_name,
                );
                result.push(GoogleGenerativeAIContentPart::Text {
                    text: reasoning_part.text.clone(),
                    thought: Some(true),
                    thought_signature: ts,
                });
            }
            AssistantContentPart::ToolCall(tool_call_part) => {
                let ts = extract_thought_signature(
                    &tool_call_part.provider_metadata,
                    provider_options_name,
                );
                result.push(GoogleGenerativeAIContentPart::FunctionCall {
                    function_call: FunctionCallPart {
                        name: tool_call_part.tool_name.clone(),
                        args: tool_call_part.input.clone(),
                    },
                    thought_signature: ts,
                });
            }
            AssistantContentPart::ToolResult(_) => {
                // Tool results in assistant messages are not sent to Google.
            }
            AssistantContentPart::Source(_) => {
                // Source parts are informational, not sent to Google.
            }
            AssistantContentPart::ToolApprovalRequest(_) => {
                // Tool approval requests are not sent to Google.
            }
            AssistantContentPart::Custom(_) => {
                // Custom parts are provider-specific, not sent to Google.
            }
        }
    }
    Ok(result)
}

fn convert_tool_content_parts(
    parts: &[ToolContentPart],
    supports_function_response_parts: bool,
) -> Vec<GoogleGenerativeAIContentPart> {
    let mut result = Vec::new();
    for part in parts {
        match part {
            ToolContentPart::ToolResult(tool_result) => {
                convert_tool_result_output(
                    &tool_result.tool_name,
                    &tool_result.output,
                    supports_function_response_parts,
                    &mut result,
                );
            }
            ToolContentPart::ToolApprovalResponse(_) => {
                // Tool approval responses are not directly sent to Google.
            }
        }
    }
    result
}

fn convert_tool_result_output(
    tool_name: &str,
    output: &ToolResultContent,
    supports_function_response_parts: bool,
    result: &mut Vec<GoogleGenerativeAIContentPart>,
) {
    match output {
        ToolResultContent::Content { value, .. } => {
            if supports_function_response_parts {
                append_tool_result_parts(tool_name, value, result);
            } else {
                append_legacy_tool_result_parts(tool_name, value, result);
            }
        }
        _ => {
            // All non-content types: wrap as { name, content }
            let content_value = match output {
                ToolResultContent::Text { value, .. } => Value::String(value.clone()),
                ToolResultContent::Json { value, .. } => value.clone(),
                ToolResultContent::ExecutionDenied { reason, .. } => Value::String(
                    reason
                        .as_deref()
                        .unwrap_or("Tool execution denied.")
                        .to_string(),
                ),
                ToolResultContent::ErrorText { value, .. } => Value::String(value.clone()),
                ToolResultContent::ErrorJson { value, .. } => value.clone(),
                ToolResultContent::Content { .. } => unreachable!(),
            };
            result.push(GoogleGenerativeAIContentPart::FunctionResponse {
                function_response: FunctionResponsePart {
                    name: tool_name.to_string(),
                    response: serde_json::json!({
                        "name": tool_name,
                        "content": content_value,
                    }),
                    parts: None,
                },
            });
        }
    }
}

/// Parse a `data:` URL into (media_type, base64_data), returning `None` for non-data URLs.
fn parse_base64_data_url(url: &str) -> Option<(String, String)> {
    let parsed = vercel_ai_provider_utils::parse_data_url(url)?;
    if !parsed.is_base64 {
        return None;
    }
    Some((parsed.media_type.unwrap_or_default(), parsed.data))
}

/// Gemini 3+ multimodal tool result: packs images/files as `functionResponse.parts`
/// alongside the text response.
fn append_tool_result_parts(
    tool_name: &str,
    parts: &[ToolResultContentPart],
    result: &mut Vec<GoogleGenerativeAIContentPart>,
) {
    let mut function_response_parts: Vec<InlineDataPart> = Vec::new();
    let mut response_text_parts: Vec<String> = Vec::new();

    for content_part in parts {
        match content_part {
            ToolResultContentPart::Text { text, .. } => {
                response_text_parts.push(text.clone());
            }
            ToolResultContentPart::FileData {
                data, media_type, ..
            } => {
                function_response_parts.push(InlineDataPart {
                    mime_type: media_type.clone(),
                    data: data.clone(),
                });
            }
            ToolResultContentPart::FileUrl { url, .. } => {
                // Only data: URLs can be converted to inline data
                if let Some((media_type, data)) = parse_base64_data_url(url) {
                    function_response_parts.push(InlineDataPart {
                        mime_type: media_type,
                        data,
                    });
                } else {
                    let json_str = serde_json::to_string(content_part).unwrap_or_default();
                    response_text_parts.push(json_str);
                }
            }
            other => {
                let json_str = serde_json::to_string(other).unwrap_or_default();
                response_text_parts.push(json_str);
            }
        }
    }

    let text_content = if response_text_parts.is_empty() {
        "Tool executed successfully.".to_string()
    } else {
        response_text_parts.join("\n")
    };

    let inline_parts = if function_response_parts.is_empty() {
        None
    } else {
        Some(function_response_parts)
    };

    let response_part = FunctionResponsePart {
        name: tool_name.to_string(),
        response: serde_json::json!({
            "name": tool_name,
            "content": text_content,
        }),
        parts: inline_parts,
    };

    result.push(GoogleGenerativeAIContentPart::FunctionResponse {
        function_response: response_part,
    });
}

/// Legacy (pre-Gemini 3) tool result: images as separate `inlineData` parts.
fn append_legacy_tool_result_parts(
    tool_name: &str,
    parts: &[ToolResultContentPart],
    result: &mut Vec<GoogleGenerativeAIContentPart>,
) {
    for content_part in parts {
        match content_part {
            ToolResultContentPart::Text { text, .. } => {
                result.push(GoogleGenerativeAIContentPart::FunctionResponse {
                    function_response: FunctionResponsePart {
                        name: tool_name.to_string(),
                        response: serde_json::json!({
                            "name": tool_name,
                            "content": text,
                        }),
                        parts: None,
                    },
                });
            }
            ToolResultContentPart::FileData {
                data, media_type, ..
            } => {
                result.push(GoogleGenerativeAIContentPart::InlineData {
                    inline_data: InlineDataPart {
                        mime_type: media_type.clone(),
                        data: data.clone(),
                    },
                    thought: None,
                    thought_signature: None,
                });
                result.push(GoogleGenerativeAIContentPart::Text {
                    text: "Tool executed successfully and returned this image as a response"
                        .to_string(),
                    thought: None,
                    thought_signature: None,
                });
            }
            other => {
                let json_str = serde_json::to_string(other).unwrap_or_default();
                result.push(GoogleGenerativeAIContentPart::Text {
                    text: json_str,
                    thought: None,
                    thought_signature: None,
                });
            }
        }
    }
}

/// Convert `SharedV4FileData` to an InlineData part (URLs are rejected by the caller beforehand).
fn convert_file_part_to_inline(
    data: &SharedV4FileData,
    media_type: &str,
) -> GoogleGenerativeAIContentPart {
    let media_type = if media_type == "image/*" {
        "image/jpeg"
    } else {
        media_type
    };
    match data {
        SharedV4FileData::Data { data: raw } => {
            let encoded = raw_file_data_to_base64(raw);
            GoogleGenerativeAIContentPart::InlineData {
                inline_data: InlineDataPart {
                    mime_type: media_type.to_string(),
                    data: encoded,
                },
                thought: None,
                thought_signature: None,
            }
        }
        SharedV4FileData::Url { url } => GoogleGenerativeAIContentPart::FileData {
            file_data: FileDataPart {
                mime_type: media_type.to_string(),
                file_uri: url.clone(),
            },
        },
        SharedV4FileData::Text { text } => {
            let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
            GoogleGenerativeAIContentPart::InlineData {
                inline_data: InlineDataPart {
                    mime_type: media_type.to_string(),
                    data: encoded,
                },
                thought: None,
                thought_signature: None,
            }
        }
        SharedV4FileData::Reference { .. } => GoogleGenerativeAIContentPart::InlineData {
            inline_data: InlineDataPart {
                mime_type: media_type.to_string(),
                data: String::new(),
            },
            thought: None,
            thought_signature: None,
        },
    }
}

/// Convert `SharedV4FileData` to a Google content part (inline data or file URI).
fn convert_file_part(data: &SharedV4FileData, media_type: &str) -> GoogleGenerativeAIContentPart {
    let media_type = if media_type == "image/*" {
        "image/jpeg"
    } else {
        media_type
    };
    match data {
        SharedV4FileData::Url { url } => GoogleGenerativeAIContentPart::FileData {
            file_data: FileDataPart {
                mime_type: media_type.to_string(),
                file_uri: url.clone(),
            },
        },
        SharedV4FileData::Data { data: raw } => {
            let encoded = raw_file_data_to_base64(raw);
            GoogleGenerativeAIContentPart::InlineData {
                inline_data: InlineDataPart {
                    mime_type: media_type.to_string(),
                    data: encoded,
                },
                thought: None,
                thought_signature: None,
            }
        }
        SharedV4FileData::Text { text } => {
            let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
            GoogleGenerativeAIContentPart::InlineData {
                inline_data: InlineDataPart {
                    mime_type: media_type.to_string(),
                    data: encoded,
                },
                thought: None,
                thought_signature: None,
            }
        }
        SharedV4FileData::Reference { .. } => GoogleGenerativeAIContentPart::InlineData {
            inline_data: InlineDataPart {
                mime_type: media_type.to_string(),
                data: String::new(),
            },
            thought: None,
            thought_signature: None,
        },
    }
}

/// Convert `LanguageModelV4FileData` (2-arm: Data/Url) to an InlineData or FileData part.
fn convert_lm_file_data_to_inline(
    data: &LanguageModelV4FileData,
    media_type: &str,
) -> GoogleGenerativeAIContentPart {
    let media_type = if media_type == "image/*" {
        "image/jpeg"
    } else {
        media_type
    };
    match data {
        LanguageModelV4FileData::Data { data: raw } => {
            let encoded = raw_file_data_to_base64(raw);
            GoogleGenerativeAIContentPart::InlineData {
                inline_data: InlineDataPart {
                    mime_type: media_type.to_string(),
                    data: encoded,
                },
                thought: None,
                thought_signature: None,
            }
        }
        LanguageModelV4FileData::Url { url } => GoogleGenerativeAIContentPart::FileData {
            file_data: FileDataPart {
                mime_type: media_type.to_string(),
                file_uri: url.clone(),
            },
        },
    }
}

fn raw_file_data_to_base64(raw: &FileRawData) -> String {
    match raw {
        FileRawData::Base64(b64) => b64.clone(),
        FileRawData::Bytes(bytes) => base64::engine::general_purpose::STANDARD.encode(bytes),
    }
}

#[cfg(test)]
#[path = "convert_to_google_generative_ai_messages.test.rs"]
mod tests;
