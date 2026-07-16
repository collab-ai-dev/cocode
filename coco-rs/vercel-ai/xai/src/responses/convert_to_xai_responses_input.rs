use base64::Engine as _;
use serde_json::Value;
use serde_json::json;
use vercel_ai_provider::AISdkError;
use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::FilePart;
use vercel_ai_provider::FileRawData;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::LanguageModelV4Prompt;
use vercel_ai_provider::ProviderMetadata;
use vercel_ai_provider::SharedV4FileData;
use vercel_ai_provider::ToolContentPart;
use vercel_ai_provider::ToolResultContent;
use vercel_ai_provider::ToolResultContentPart;
use vercel_ai_provider::UserContentPart;
use vercel_ai_provider::Warning;

/// Convert a `LanguageModelV4Prompt` into xAI Responses API input items.
///
/// Mirrors `convert-to-xai-responses-input.ts`. Returns `Ok((input, warnings))`.
/// Only `text file parts` and inline non-image `data` produce hard errors; every
/// other unsupported shape degrades to an `other` warning.
pub fn convert_to_xai_responses_input(
    prompt: &LanguageModelV4Prompt,
) -> Result<(Vec<Value>, Vec<Warning>), AISdkError> {
    let mut input: Vec<Value> = Vec::new();
    let mut warnings: Vec<Warning> = Vec::new();

    for msg in prompt {
        match msg {
            LanguageModelV4Message::System { content, .. } => {
                let text = collapse_text_parts(content, &mut warnings, "system message");
                input.push(json!({ "role": "system", "content": text }));
            }
            LanguageModelV4Message::Developer { content, .. } => {
                let text = collapse_text_parts(content, &mut warnings, "developer message");
                input.push(json!({ "role": "developer", "content": text }));
            }

            LanguageModelV4Message::User { content, .. } => {
                let mut parts: Vec<Value> = Vec::new();
                for part in content {
                    match part {
                        UserContentPart::Text(t) => {
                            parts.push(json!({ "type": "input_text", "text": t.text }));
                        }
                        UserContentPart::File(file) => {
                            convert_user_file_part(file, &mut parts, &mut warnings)?;
                        }
                    }
                }
                input.push(json!({ "role": "user", "content": parts }));
            }

            LanguageModelV4Message::Assistant { content, .. } => {
                for part in content {
                    convert_assistant_part(part, &mut input, &mut warnings);
                }
            }

            LanguageModelV4Message::Tool { content, .. } => {
                for part in content {
                    if let ToolContentPart::ToolResult(result) = part {
                        let output = serialize_tool_result_output(&result.output);
                        input.push(json!({
                            "type": "function_call_output",
                            "call_id": result.tool_call_id,
                            "output": output,
                        }));
                    }
                }
            }
        }
    }

    Ok((input, warnings))
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
            UserContentPart::File(_) => warnings.push(Warning::other(format!(
                "{context} contains a non-text part that was dropped"
            ))),
        }
    }
    text
}

/// Convert a user `file` content part into the appropriate Responses input part.
fn convert_user_file_part(
    file: &FilePart,
    parts: &mut Vec<Value>,
    warnings: &mut Vec<Warning>,
) -> Result<(), AISdkError> {
    match &file.data {
        SharedV4FileData::Reference { reference } => {
            match reference.get("xai").or_else(|| reference.values().next()) {
                Some(file_id) => {
                    parts.push(json!({ "type": "input_file", "file_id": file_id }));
                }
                None => warnings.push(Warning::other(
                    "xAI Responses API file reference has no id and was dropped",
                )),
            }
            Ok(())
        }
        SharedV4FileData::Text { .. } => Err(AISdkError::new(
            "Unsupported functionality: text file parts",
        )),
        SharedV4FileData::Url { url } => {
            if is_image_media_type(&file.media_type) {
                let mut image = json!({ "type": "input_image", "image_url": url });
                if let Some(detail) = xai_image_detail(file) {
                    image["detail"] = Value::String(detail);
                }
                parts.push(image);
            } else {
                // Non-image documents pass through as a URL-referenced input_file.
                parts.push(json!({ "type": "input_file", "file_url": url }));
            }
            Ok(())
        }
        SharedV4FileData::Data { data } => {
            if is_image_media_type(&file.media_type) {
                let b64 = file_raw_data_to_base64(data);
                let full_type = full_media_type(&file.media_type);
                let image_url = format!("data:{full_type};base64,{b64}");
                let mut image = json!({ "type": "input_image", "image_url": image_url });
                if let Some(detail) = xai_image_detail(file) {
                    image["detail"] = Value::String(detail);
                }
                parts.push(image);
                Ok(())
            } else {
                // Inline bytes for non-image files are not supported by xAI;
                // callers must upload via the Files API and pass a reference.
                Err(AISdkError::new(format!(
                    "Unsupported functionality: file part media type {} as inline data (xAI Responses requires a URL or a Files API reference for non-image files)",
                    file.media_type
                )))
            }
        }
    }
}

/// Convert an assistant content part into zero or more Responses input items.
fn convert_assistant_part(
    part: &AssistantContentPart,
    input: &mut Vec<Value>,
    warnings: &mut Vec<Warning>,
) {
    match part {
        AssistantContentPart::Text(t) => {
            let mut msg = json!({ "role": "assistant", "content": t.text });
            if let Some(id) = xai_meta_str(&t.provider_metadata, "itemId") {
                msg["id"] = Value::String(id);
            }
            input.push(msg);
        }
        AssistantContentPart::ToolCall(tc) => {
            if tc.provider_executed == Some(true) {
                return;
            }
            let id = xai_meta_str(&tc.provider_metadata, "itemId")
                .unwrap_or_else(|| tc.tool_call_id.clone());
            let arguments = serde_json::to_string(&tc.input).unwrap_or_default();
            input.push(json!({
                "type": "function_call",
                "id": id,
                "call_id": tc.tool_call_id,
                "name": tc.tool_name,
                "arguments": arguments,
            }));
        }
        AssistantContentPart::Reasoning(rp) => {
            let item_id = xai_meta_str(&rp.provider_metadata, "itemId");
            let encrypted = xai_meta_str(&rp.provider_metadata, "reasoningEncryptedContent");
            if item_id.is_none() && encrypted.is_none() {
                warnings.push(Warning::other(
                    "Reasoning parts without itemId or encrypted content cannot be sent back to xAI. Skipping.",
                ));
                return;
            }
            let mut summary: Vec<Value> = Vec::new();
            if !rp.text.is_empty() {
                summary.push(json!({ "type": "summary_text", "text": rp.text }));
            }
            let mut item = json!({
                "type": "reasoning",
                "id": item_id.unwrap_or_default(),
                "summary": summary,
            });
            if let Some(enc) = encrypted {
                item["encrypted_content"] = Value::String(enc);
            }
            input.push(item);
        }
        // Tool results are folded into the tool message; every remaining
        // assistant part has no Responses input representation.
        AssistantContentPart::ToolResult(_) | AssistantContentPart::Source(_) => {}
        AssistantContentPart::ReasoningFile(_)
        | AssistantContentPart::Custom(_)
        | AssistantContentPart::File(_) => {
            warnings.push(Warning::other(
                "xAI Responses API does not support this content type in assistant messages",
            ));
        }
        AssistantContentPart::ToolApprovalRequest(_) => {}
    }
}

/// Read a string field off a part's `provider_metadata["xai"]` namespace.
fn xai_meta_str(pm: &Option<ProviderMetadata>, key: &str) -> Option<String> {
    pm.as_ref()?
        .0
        .get("xai")?
        .get(key)?
        .as_str()
        .map(String::from)
}

/// Read the xAI `imageDetail` provider option (`low` | `high` | `auto`).
fn xai_image_detail(file: &FilePart) -> Option<String> {
    let detail = xai_meta_str(&file.provider_metadata, "imageDetail")?;
    matches!(detail.as_str(), "low" | "high" | "auto").then_some(detail)
}

/// Whether a media type's top-level segment is `image`.
fn is_image_media_type(media_type: &str) -> bool {
    media_type
        .split('/')
        .next()
        .is_some_and(|top| top == "image")
}

/// Resolve `image/*` to a concrete default type for a `data:` URI.
fn full_media_type(media_type: &str) -> &str {
    if media_type == "image/*" {
        "image/jpeg"
    } else {
        media_type
    }
}

fn file_raw_data_to_base64(raw: &FileRawData) -> String {
    match raw {
        FileRawData::Base64(b64) => b64.clone(),
        FileRawData::Bytes(bytes) => base64::engine::general_purpose::STANDARD.encode(bytes),
    }
}

/// Stringify a tool result output for the `function_call_output.output` field.
///
/// Mirrors `convert-to-xai-responses-input.ts`: multimodal `content` outputs
/// keep only the text parts (non-text collapse to an empty string), unlike the
/// chat converter's visible marker.
fn serialize_tool_result_output(output: &ToolResultContent) -> String {
    match output {
        ToolResultContent::Text { value, .. } | ToolResultContent::ErrorText { value, .. } => {
            value.clone()
        }
        ToolResultContent::ExecutionDenied { reason, .. } => reason
            .clone()
            .unwrap_or_else(|| "tool execution denied".to_string()),
        ToolResultContent::Json { value, .. } | ToolResultContent::ErrorJson { value, .. } => {
            serde_json::to_string(value).unwrap_or_default()
        }
        ToolResultContent::Content { value, .. } => value
            .iter()
            .map(|part| match part {
                ToolResultContentPart::Text { text, .. } => text.as_str(),
                _ => "",
            })
            .collect::<String>(),
    }
}

#[cfg(test)]
#[path = "convert_to_xai_responses_input.test.rs"]
mod tests;
