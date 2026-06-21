use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;
use std::collections::HashSet;
use tracing::debug;
use tracing::warn;
use vercel_ai_provider::AISdkError;
use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::FileRawData;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::LanguageModelV4Prompt;
use vercel_ai_provider::ProviderMetadata;
use vercel_ai_provider::SharedV4FileData;
use vercel_ai_provider::ToolContentPart;
use vercel_ai_provider::ToolResultContent;
use vercel_ai_provider::UserContentPart;
use vercel_ai_provider::Warning;

use crate::provider_profile::OpenAICompatibleProviderProfile;

/// Convert a `LanguageModelV4Prompt` into OpenAI-compatible Chat Completions API messages.
///
/// Unlike the OpenAI-specific converter, this always uses `role: "system"` (no Developer mode)
/// and includes `reasoning_content` in assistant messages.
///
/// Returns `Ok((messages, warnings))`.
pub fn convert_to_openai_compatible_chat_messages(
    prompt: &LanguageModelV4Prompt,
) -> Result<(Vec<Value>, Vec<Warning>), AISdkError> {
    convert_to_openai_compatible_chat_messages_with_profile(
        prompt,
        OpenAICompatibleProviderProfile::Generic,
    )
}

pub(crate) fn convert_to_openai_compatible_chat_messages_with_profile(
    prompt: &LanguageModelV4Prompt,
    profile: OpenAICompatibleProviderProfile,
) -> Result<(Vec<Value>, Vec<Warning>), AISdkError> {
    let mut messages = Vec::new();
    let mut warnings = Vec::new();
    let mut deepseek_pairing = if profile.is_deepseek() {
        Some(DeepSeekToolPairing::default())
    } else {
        None
    };
    let mut generic_diagnostics = if profile.is_deepseek() {
        None
    } else {
        Some(GenericToolPairingDiagnostics::new(profile))
    };

    for msg in prompt {
        match msg {
            LanguageModelV4Message::System {
                content,
                provider_options,
            } => {
                flush_deepseek_pairing(&mut deepseek_pairing, &mut messages);
                flush_generic_diagnostics(&mut generic_diagnostics);
                let text = collapse_text_parts(content, &mut warnings, "system message");
                let mut msg = json!({ "role": "system", "content": text });
                for (k, v) in get_openai_metadata(provider_options) {
                    msg[k] = v;
                }
                messages.push(msg);
            }

            LanguageModelV4Message::Developer {
                content,
                provider_options,
            } => {
                flush_deepseek_pairing(&mut deepseek_pairing, &mut messages);
                flush_generic_diagnostics(&mut generic_diagnostics);
                let text = collapse_text_parts(content, &mut warnings, "developer message");
                let mut msg = json!({ "role": "developer", "content": text });
                for (k, v) in get_openai_metadata(provider_options) {
                    msg[k] = v;
                }
                messages.push(msg);
            }

            LanguageModelV4Message::User {
                content,
                provider_options,
            } => {
                flush_deepseek_pairing(&mut deepseek_pairing, &mut messages);
                flush_generic_diagnostics(&mut generic_diagnostics);
                let parts = convert_user_parts(content, provider_options)?;
                // Single text part can be simplified to just a string
                if parts.len() == 1 && parts[0].get("type").and_then(|t| t.as_str()) == Some("text")
                {
                    let mut msg = json!({
                        "role": "user",
                        "content": parts[0]["text"]
                    });
                    // For single text, spread the part's metadata on the message
                    if let Some(UserContentPart::Text(text_part)) = content.first() {
                        for (k, v) in get_part_metadata(&text_part.provider_metadata) {
                            msg[k] = v;
                        }
                    }
                    messages.push(msg);
                } else {
                    let mut msg = json!({
                        "role": "user",
                        "content": parts
                    });
                    // For multi-part, spread message-level metadata on the message
                    for (k, v) in get_openai_metadata(provider_options) {
                        msg[k] = v;
                    }
                    messages.push(msg);
                }
            }

            LanguageModelV4Message::Assistant {
                content,
                provider_options,
            } => {
                flush_deepseek_pairing(&mut deepseek_pairing, &mut messages);
                flush_generic_diagnostics(&mut generic_diagnostics);
                let (text, tool_calls, reasoning_content, call_infos) =
                    convert_assistant_parts(content, profile);
                let mut msg = json!({ "role": "assistant" });
                if let Some(text) = text {
                    msg["content"] = Value::String(text);
                }
                if !tool_calls.is_empty() {
                    if profile.is_deepseek() && msg.get("content").is_none() {
                        msg["content"] = Value::Null;
                    }
                    msg["tool_calls"] = Value::Array(tool_calls);
                }
                // Include reasoning_content in assistant messages for providers that support it
                if let Some(reasoning) = reasoning_content
                    && (!profile.is_deepseek() || msg.get("tool_calls").is_some())
                {
                    msg["reasoning_content"] = Value::String(reasoning);
                }
                for (k, v) in get_openai_metadata(provider_options) {
                    msg[k] = v;
                }
                messages.push(msg);
                if let Some(pairing) = deepseek_pairing.as_mut()
                    && !call_infos.is_empty()
                {
                    pairing.start(call_infos);
                } else if let Some(diagnostics) = generic_diagnostics.as_mut()
                    && !call_infos.is_empty()
                {
                    diagnostics.start(call_infos);
                }
            }

            LanguageModelV4Message::Tool {
                content,
                provider_options: _,
            } => {
                if let Some(pairing) = deepseek_pairing.as_mut() {
                    pairing.accept_tool_message(content);
                    continue;
                }
                if let Some(diagnostics) = generic_diagnostics.as_mut() {
                    diagnostics.accept_tool_message(content);
                }
                for part in content {
                    match part {
                        ToolContentPart::ToolResult(result) => {
                            let output = serialize_tool_result_content(&result.output);
                            let mut msg = json!({
                                "role": "tool",
                                "tool_call_id": result.tool_call_id,
                                "content": output,
                            });
                            for (k, v) in get_part_metadata(&result.provider_metadata) {
                                msg[k] = v;
                            }
                            messages.push(msg);
                        }
                        ToolContentPart::ToolApprovalResponse(_) => {
                            // Approval responses are not supported in Chat API
                        }
                    }
                }
            }
        }
    }

    flush_deepseek_pairing(&mut deepseek_pairing, &mut messages);
    flush_generic_diagnostics(&mut generic_diagnostics);

    Ok((messages, warnings))
}

fn flush_deepseek_pairing(pairing: &mut Option<DeepSeekToolPairing>, messages: &mut Vec<Value>) {
    if let Some(pairing) = pairing.as_mut() {
        pairing.flush(messages);
    }
}

fn flush_generic_diagnostics(pairing: &mut Option<GenericToolPairingDiagnostics>) {
    if let Some(pairing) = pairing.as_mut() {
        pairing.flush();
    }
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

/// Extract provider metadata from the "openaiCompatible" key in provider options.
fn get_openai_metadata(
    provider_options: &Option<vercel_ai_provider::ProviderOptions>,
) -> serde_json::Map<String, Value> {
    provider_options
        .as_ref()
        .and_then(|opts| opts.0.get("openaiCompatible"))
        .map(|inner| inner.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}

/// Extract provider metadata from the "openaiCompatible" key in a content part's provider metadata.
fn get_part_metadata(pm: &Option<ProviderMetadata>) -> serde_json::Map<String, Value> {
    pm.as_ref()
        .and_then(|meta| meta.0.get("openaiCompatible"))
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default()
}

/// Convert user content parts to OpenAI-compatible format.
fn convert_user_parts(
    parts: &[UserContentPart],
    provider_options: &Option<vercel_ai_provider::ProviderOptions>,
) -> Result<Vec<Value>, AISdkError> {
    // Extract imageDetail from any provider options (generic key lookup)
    let image_detail = provider_options.as_ref().and_then(|opts| {
        // Try to find imageDetail in any provider's options
        opts.0.values().find_map(|v| {
            v.get("imageDetail")
                .and_then(|v| v.as_str())
                .map(String::from)
        })
    });

    parts
        .iter()
        .map(|part| match part {
            UserContentPart::Text(text_part) => {
                let mut val = json!({ "type": "text", "text": text_part.text });
                for (k, v) in get_part_metadata(&text_part.provider_metadata) {
                    val[k] = v;
                }
                Ok(val)
            }
            UserContentPart::File(file_part) => {
                let media_type = &file_part.media_type;
                let part_meta = get_part_metadata(&file_part.provider_metadata);

                if media_type.starts_with("image/") {
                    // #16: Convert image/* to image/jpeg as fallback
                    let effective_type = if media_type == "image/*" {
                        "image/jpeg"
                    } else {
                        media_type.as_str()
                    };
                    let url = shared_file_data_to_url(&file_part.data, effective_type);
                    let mut image_url = json!({ "url": url });
                    if let Some(ref detail) = image_detail {
                        image_url["detail"] = Value::String(detail.clone());
                    }
                    let mut val = json!({ "type": "image_url", "image_url": image_url });
                    for (k, v) in part_meta {
                        val[k] = v;
                    }
                    Ok(val)
                } else if media_type.starts_with("audio/") {
                    // Audio parts with URLs are not supported
                    if matches!(file_part.data, SharedV4FileData::Url { .. }) {
                        return Err(AISdkError::new(
                            "Unsupported functionality: audio file parts with URLs",
                        ));
                    }
                    let format = match media_type.as_str() {
                        "audio/wav" => "wav",
                        "audio/mp3" | "audio/mpeg" => "mp3",
                        _ => {
                            return Err(AISdkError::new(format!(
                                "Unsupported functionality: audio media type {media_type}"
                            )));
                        }
                    };
                    let b64 = shared_file_data_to_base64(&file_part.data);
                    let mut val = json!({
                        "type": "input_audio",
                        "input_audio": { "data": b64, "format": format }
                    });
                    for (k, v) in part_meta {
                        val[k] = v;
                    }
                    Ok(val)
                } else if media_type == "application/pdf" {
                    // PDF parts with URLs are not supported
                    if matches!(file_part.data, SharedV4FileData::Url { .. }) {
                        return Err(AISdkError::new(
                            "Unsupported functionality: PDF file parts with URLs",
                        ));
                    }
                    let b64 = shared_file_data_to_base64(&file_part.data);
                    let mut val = json!({
                        "type": "file",
                        "file": {
                            "filename": "document.pdf",
                            "file_data": format!("data:{media_type};base64,{b64}"),
                        }
                    });
                    for (k, v) in part_meta {
                        val[k] = v;
                    }
                    Ok(val)
                } else if media_type.starts_with("text/") {
                    let text = shared_file_data_to_text(&file_part.data);
                    let mut val = json!({ "type": "text", "text": text });
                    for (k, v) in part_meta {
                        val[k] = v;
                    }
                    Ok(val)
                } else {
                    Err(AISdkError::new(format!(
                        "Unsupported functionality: file part media type {media_type}"
                    )))
                }
            }
        })
        .collect()
}

/// Convert assistant content parts to (concatenated text, tool_calls array, reasoning_content).
fn convert_assistant_parts(
    parts: &[AssistantContentPart],
    profile: OpenAICompatibleProviderProfile,
) -> (
    Option<String>,
    Vec<Value>,
    Option<String>,
    Vec<DeepSeekToolCallInfo>,
) {
    let mut text_parts = Vec::new();
    let deepseek_ids = profile
        .is_deepseek()
        .then(|| DeepSeekToolCallIds::new(parts));
    let mut tool_calls = Vec::new();
    let mut call_infos = Vec::new();
    let mut reasoning_parts = Vec::new();
    let mut tool_call_idx = 0usize;

    for part in parts {
        match part {
            AssistantContentPart::Text(text_part) => {
                text_parts.push(text_part.text.clone());
            }
            AssistantContentPart::ToolCall(tc) => {
                let raw_id = tc.tool_call_id.clone();
                let (wire_id, match_kind) = if let Some(ids) = &deepseek_ids {
                    ids.normalize(tool_call_idx, &raw_id)
                } else {
                    (raw_id.clone(), DeepSeekToolCallMatchKind::Exact)
                };
                if profile.is_deepseek() && match_kind == DeepSeekToolCallMatchKind::Positional {
                    let reason = if raw_id.is_empty() {
                        "empty_id"
                    } else {
                        "duplicate_id"
                    };
                    debug!(
                        profile = "deepseek",
                        tool_name = %tc.tool_name,
                        raw_tool_call_id = %raw_id,
                        wire_tool_call_id = %wire_id,
                        tool_call_index = tool_call_idx,
                        reason,
                        "normalized DeepSeek assistant tool call id"
                    );
                }
                let arguments = serialize_tool_call_arguments(&tc.input, profile);
                let mut tool_call = json!({
                    "id": wire_id,
                    "type": "function",
                    "function": {
                        "name": tc.tool_name,
                        "arguments": arguments,
                    }
                });

                // Spread openaiCompatible part metadata
                for (k, v) in get_part_metadata(&tc.provider_metadata) {
                    tool_call[k] = v;
                }

                // #4: Include thought_signature as extra_content for Google
                // (after partMetadata so it overrides if conflicting)
                if let Some(ref pm) = tc.provider_metadata
                    && let Some(google) = pm.0.get("google")
                    && let Some(ts) = google.get("thoughtSignature").and_then(|v| v.as_str())
                {
                    tool_call["extra_content"] = json!({
                        "google": {
                            "thought_signature": ts
                        }
                    });
                }

                tool_calls.push(tool_call);
                call_infos.push(DeepSeekToolCallInfo {
                    raw_id,
                    wire_id,
                    tool_name: tc.tool_name.clone(),
                    match_kind,
                });
                tool_call_idx += 1;
            }
            AssistantContentPart::Reasoning(rp) => {
                reasoning_parts.push(rp.text.clone());
            }
            // File, Source, ToolResult, ToolApprovalRequest — skip
            _ => {}
        }
    }

    let text = if text_parts.is_empty() {
        None
    } else {
        Some(text_parts.join(""))
    };

    let reasoning = if reasoning_parts.is_empty() {
        None
    } else {
        Some(reasoning_parts.join(""))
    };

    (text, tool_calls, reasoning, call_infos)
}

struct DeepSeekToolCallIds {
    raw_id_counts: HashMap<String, usize>,
    synthetic_wire_ids: HashMap<usize, String>,
}

impl DeepSeekToolCallIds {
    fn new(parts: &[AssistantContentPart]) -> Self {
        let raw_ids: Vec<String> = parts
            .iter()
            .filter_map(|part| match part {
                AssistantContentPart::ToolCall(tc) => Some(tc.tool_call_id.clone()),
                _ => None,
            })
            .collect();
        let mut raw_id_counts = HashMap::new();
        for raw_id in &raw_ids {
            *raw_id_counts.entry(raw_id.clone()).or_insert(0) += 1;
        }

        let mut taken: HashSet<String> = raw_ids
            .iter()
            .filter(|id| !id.is_empty())
            .cloned()
            .collect();
        let synthetic_wire_ids = raw_ids
            .iter()
            .enumerate()
            .filter(|(_, raw_id)| {
                raw_id.is_empty() || raw_id_counts.get(*raw_id).copied().unwrap_or(0) > 1
            })
            .map(|(index, _)| {
                (
                    index,
                    allocate_deepseek_synthetic_wire_id(index, &mut taken),
                )
            })
            .collect();

        Self {
            raw_id_counts,
            synthetic_wire_ids,
        }
    }

    fn normalize(&self, index: usize, raw_id: &str) -> (String, DeepSeekToolCallMatchKind) {
        if !raw_id.is_empty() && self.raw_id_counts.get(raw_id).copied() == Some(1) {
            return (raw_id.to_string(), DeepSeekToolCallMatchKind::Exact);
        }

        let wire_id = self
            .synthetic_wire_ids
            .get(&index)
            .cloned()
            .unwrap_or_else(|| format!("__deepseek_call_{index}"));
        (wire_id, DeepSeekToolCallMatchKind::Positional)
    }
}

fn allocate_deepseek_synthetic_wire_id(index: usize, taken: &mut HashSet<String>) -> String {
    let mut suffix = index;
    loop {
        let candidate = format!("__deepseek_call_{suffix}");
        if taken.insert(candidate.clone()) {
            return candidate;
        }
        suffix += 1;
    }
}

fn serialize_tool_call_arguments(
    input: &Value,
    profile: OpenAICompatibleProviderProfile,
) -> String {
    if !profile.is_deepseek() {
        return serde_json::to_string(input).unwrap_or_default();
    }
    match input {
        Value::String(raw) => {
            let repaired = vercel_ai_provider_utils::parse_tool_arguments_or_empty(raw, "tool");
            if matches!(repaired, Value::String(_)) {
                "{}".to_string()
            } else {
                serde_json::to_string(&repaired).unwrap_or_else(|_| "{}".to_string())
            }
        }
        other => serde_json::to_string(other).unwrap_or_else(|_| "{}".to_string()),
    }
}

#[derive(Debug, Clone)]
struct DeepSeekToolCallInfo {
    raw_id: String,
    wire_id: String,
    tool_name: String,
    match_kind: DeepSeekToolCallMatchKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeepSeekToolCallMatchKind {
    Exact,
    Positional,
}

#[derive(Debug, Clone)]
struct PendingToolResult {
    output: String,
    provider_metadata: Option<ProviderMetadata>,
}

#[derive(Debug, Default)]
struct DeepSeekToolPairing {
    calls: Vec<DeepSeekToolCallInfo>,
    results: Vec<Option<PendingToolResult>>,
    next_positional_index: usize,
}

impl DeepSeekToolPairing {
    fn start(&mut self, calls: Vec<DeepSeekToolCallInfo>) {
        self.calls = calls;
        self.results = vec![None; self.calls.len()];
        self.next_positional_index = 0;
    }

    fn accept_tool_message(&mut self, content: &[ToolContentPart]) {
        if self.calls.is_empty() {
            return;
        }
        for part in content {
            let ToolContentPart::ToolResult(result) = part else {
                continue;
            };
            let Some(index) = self.match_result_index(&result.tool_call_id) else {
                warn!(
                    profile = "deepseek",
                    tool_name = %result.tool_name,
                    raw_tool_result_id = %result.tool_call_id,
                    pending_tool_call_count = self.calls.len(),
                    "dropping unmatched DeepSeek tool result"
                );
                continue;
            };
            self.results[index] = Some(PendingToolResult {
                output: serialize_tool_result_content(&result.output),
                provider_metadata: result.provider_metadata.clone(),
            });
        }
    }

    fn match_result_index(&mut self, raw_result_id: &str) -> Option<usize> {
        if !raw_result_id.is_empty() {
            if let Some(index) = self.calls.iter().position(|call| {
                call.match_kind == DeepSeekToolCallMatchKind::Exact && call.raw_id == raw_result_id
            }) {
                return self
                    .results
                    .get(index)
                    .is_some_and(Option::is_none)
                    .then_some(index);
            }

            if !self.calls.iter().any(|call| {
                call.match_kind == DeepSeekToolCallMatchKind::Positional
                    && call.raw_id == raw_result_id
            }) {
                return None;
            }
        }

        while self.next_positional_index < self.calls.len() {
            let index = self.next_positional_index;
            self.next_positional_index += 1;
            if self.calls[index].match_kind == DeepSeekToolCallMatchKind::Positional
                && self.results[index].is_none()
            {
                let call = &self.calls[index];
                debug!(
                    profile = "deepseek",
                    tool_name = %call.tool_name,
                    raw_tool_call_id = %call.raw_id,
                    raw_tool_result_id = %raw_result_id,
                    wire_tool_call_id = %call.wire_id,
                    tool_call_index = index,
                    "matched DeepSeek tool result positionally"
                );
                return Some(index);
            }
        }
        None
    }

    fn flush(&mut self, messages: &mut Vec<Value>) {
        if self.calls.is_empty() {
            return;
        }
        for (index, call) in self.calls.iter().enumerate() {
            let result = self.results[index].take().unwrap_or_else(|| {
                warn!(
                    profile = "deepseek",
                    tool_name = %call.tool_name,
                    raw_tool_call_id = %call.raw_id,
                    wire_tool_call_id = %call.wire_id,
                    tool_call_index = index,
                    "synthesizing missing DeepSeek tool result"
                );
                PendingToolResult {
                    output: format!(
                        "<tool_use_error>Tool result missing for {}</tool_use_error>",
                        call.tool_name
                    ),
                    provider_metadata: None,
                }
            });
            let mut msg = json!({
                "role": "tool",
                "tool_call_id": call.wire_id,
                "content": result.output,
            });
            for (k, v) in get_part_metadata(&result.provider_metadata) {
                msg[k] = v;
            }
            messages.push(msg);
        }
        self.calls.clear();
        self.results.clear();
        self.next_positional_index = 0;
    }
}

#[derive(Debug)]
struct GenericToolPairingDiagnostics {
    profile: OpenAICompatibleProviderProfile,
    calls: Vec<DiagnosticToolCall>,
}

#[derive(Debug)]
struct DiagnosticToolCall {
    raw_id: String,
    tool_name: String,
    has_result: bool,
}

impl GenericToolPairingDiagnostics {
    fn new(profile: OpenAICompatibleProviderProfile) -> Self {
        Self {
            profile,
            calls: Vec::new(),
        }
    }

    fn start(&mut self, calls: Vec<DeepSeekToolCallInfo>) {
        self.calls = calls
            .into_iter()
            .map(|call| DiagnosticToolCall {
                raw_id: call.raw_id,
                tool_name: call.tool_name,
                has_result: false,
            })
            .collect();
        self.log_assistant_tool_call_id_anomalies();
    }

    fn accept_tool_message(&mut self, content: &[ToolContentPart]) {
        for part in content {
            let ToolContentPart::ToolResult(result) = part else {
                continue;
            };
            let Some(index) = self
                .calls
                .iter()
                .position(|call| call.raw_id == result.tool_call_id && !call.has_result)
            else {
                debug!(
                    profile = ?self.profile,
                    tool_name = %result.tool_name,
                    raw_tool_result_id = %result.tool_call_id,
                    pending_tool_call_count = self.calls.len(),
                    "observed unmatched OpenAI-compatible tool result"
                );
                continue;
            };
            self.calls[index].has_result = true;
        }
    }

    fn flush(&mut self) {
        for (index, call) in self.calls.iter().enumerate() {
            if !call.has_result {
                debug!(
                    profile = ?self.profile,
                    tool_name = %call.tool_name,
                    raw_tool_call_id = %call.raw_id,
                    tool_call_index = index,
                    "observed OpenAI-compatible assistant tool call without following tool result"
                );
            }
        }
        self.calls.clear();
    }

    fn log_assistant_tool_call_id_anomalies(&self) {
        let mut id_counts = HashMap::new();
        for call in &self.calls {
            *id_counts.entry(call.raw_id.as_str()).or_insert(0usize) += 1;
        }
        for (index, call) in self.calls.iter().enumerate() {
            if call.raw_id.is_empty() {
                debug!(
                    profile = ?self.profile,
                    tool_name = %call.tool_name,
                    tool_call_index = index,
                    "observed OpenAI-compatible assistant tool call with empty id"
                );
            } else if id_counts.get(call.raw_id.as_str()).copied().unwrap_or(0) > 1 {
                debug!(
                    profile = ?self.profile,
                    tool_name = %call.tool_name,
                    raw_tool_call_id = %call.raw_id,
                    tool_call_index = index,
                    "observed OpenAI-compatible assistant tool call with duplicate id"
                );
            }
        }
    }
}

/// Serialize a tool result content to a string for the Chat API.
fn serialize_tool_result_content(content: &ToolResultContent) -> String {
    match content {
        ToolResultContent::Text { value, .. } => value.clone(),
        ToolResultContent::Json { value, .. } => serde_json::to_string(value).unwrap_or_default(),
        ToolResultContent::ErrorText { value, .. } => value.clone(),
        ToolResultContent::ErrorJson { value, .. } => {
            serde_json::to_string(value).unwrap_or_default()
        }
        ToolResultContent::ExecutionDenied { reason, .. } => {
            reason.clone().unwrap_or_else(|| "Execution denied".into())
        }
        ToolResultContent::Content { value, .. } => {
            // OpenAI-Compatible providers (DeepSeek / xAI / Groq /
            // Together / …) inherit OpenAI Chat Completions' single-
            // string `tool` role message — they can't carry image or
            // document blocks. Match the OpenAI chat degradation: pass
            // Text parts through, replace non-Text parts with a
            // visible marker so the model knows *something* was there.
            // Pre-refactor this branch JSON-stringified the whole Vec
            // (model saw `[{"type":"file-data","data":"iVBOR..."}]`),
            // a leak that wasted tokens with no upside.
            use vercel_ai_provider::ToolResultContentPart;
            let parts: Vec<String> = value
                .iter()
                .map(|part| match part {
                    ToolResultContentPart::Text { text, .. } => text.clone(),
                    ToolResultContentPart::FileData { media_type, .. }
                    | ToolResultContentPart::FileUrl { media_type, .. } => format!(
                        "[{media_type} content omitted — provider doesn't support multimodal tool results]"
                    ),
                    ToolResultContentPart::FileReference { .. } => {
                        "[file reference omitted — provider doesn't support multimodal tool results]"
                            .into()
                    }
                    ToolResultContentPart::Custom { .. } => {
                        "[custom provider-specific content omitted]".into()
                    }
                })
                .collect();
            parts.join("\n")
        }
    }
}

fn shared_file_data_to_url(data: &SharedV4FileData, media_type: &str) -> String {
    match data {
        SharedV4FileData::Url { url } => url.clone(),
        SharedV4FileData::Data { data: raw } => {
            let b64 = file_raw_data_to_base64(raw);
            format!("data:{media_type};base64,{b64}")
        }
        SharedV4FileData::Text { text } => {
            use base64::Engine as _;
            let b64 = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
            format!("data:{media_type};base64,{b64}")
        }
        SharedV4FileData::Reference { .. } => String::new(),
    }
}

fn shared_file_data_to_base64(data: &SharedV4FileData) -> String {
    match data {
        SharedV4FileData::Data { data: raw } => file_raw_data_to_base64(raw),
        SharedV4FileData::Url { url } => {
            if let Some(idx) = url.find(";base64,") {
                url[idx + 8..].to_string()
            } else {
                url.clone()
            }
        }
        SharedV4FileData::Text { text } => {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(text.as_bytes())
        }
        SharedV4FileData::Reference { .. } => String::new(),
    }
}

fn shared_file_data_to_text(data: &SharedV4FileData) -> String {
    match data {
        SharedV4FileData::Text { text } => text.clone(),
        SharedV4FileData::Data { data: raw } => match raw {
            FileRawData::Base64(b64) => {
                use base64::Engine as _;
                base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .ok()
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .unwrap_or_default()
            }
            FileRawData::Bytes(bytes) => String::from_utf8_lossy(bytes).into_owned(),
        },
        SharedV4FileData::Url { url } => url.clone(),
        SharedV4FileData::Reference { .. } => String::new(),
    }
}

fn file_raw_data_to_base64(raw: &FileRawData) -> String {
    match raw {
        FileRawData::Base64(b64) => b64.clone(),
        FileRawData::Bytes(bytes) => {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(bytes)
        }
    }
}

#[cfg(test)]
#[path = "convert_to_chat_messages.test.rs"]
mod tests;
