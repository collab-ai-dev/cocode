use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt;

use vercel_ai_provider::LanguageModelV4StreamPart;

/// Assigns a stable, request-local identity to every tool call.
///
/// Some providers reuse their wire ID for multiple sequential calls. The
/// effective ID must be fixed before stream events and the final snapshot are
/// emitted so every downstream map observes the same identity.
#[derive(Default)]
pub(crate) struct ToolCallIdNormalizer {
    active: HashMap<String, ActiveToolCall>,
    latest_effective: HashMap<String, String>,
    next_suffix: HashMap<String, u64>,
    used_effective: HashSet<String>,
}

struct ActiveToolCall {
    effective_id: String,
    tool_name: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ToolCallIdError {
    raw_id: String,
    active_tool_name: String,
    incoming_tool_name: String,
}

impl fmt::Display for ToolCallIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "provider emitted overlapping tool calls with id {:?}: active tool {:?}, incoming tool {:?}",
            self.raw_id, self.active_tool_name, self.incoming_tool_name
        )
    }
}

impl ToolCallIdNormalizer {
    pub(crate) fn normalize(
        &mut self,
        part: &mut LanguageModelV4StreamPart,
    ) -> Result<(), ToolCallIdError> {
        match part {
            LanguageModelV4StreamPart::ToolInputStart { id, tool_name, .. } => {
                let raw_id = id.clone();
                if let Some(active) = self.active.get(&raw_id) {
                    return Err(ToolCallIdError {
                        raw_id,
                        active_tool_name: active.tool_name.clone(),
                        incoming_tool_name: tool_name.clone(),
                    });
                }

                let effective_id = self.allocate(&raw_id);
                self.latest_effective
                    .insert(raw_id.clone(), effective_id.clone());
                self.active.insert(
                    raw_id,
                    ActiveToolCall {
                        effective_id: effective_id.clone(),
                        tool_name: tool_name.clone(),
                    },
                );
                *id = effective_id;
            }
            LanguageModelV4StreamPart::ToolInputDelta { id, .. }
            | LanguageModelV4StreamPart::ToolInputEnd { id, .. } => {
                if let Some(active) = self.active.get(id) {
                    *id = active.effective_id.clone();
                }
            }
            LanguageModelV4StreamPart::ToolCall(tool_call) => {
                let raw_id = tool_call.tool_call_id.clone();
                let effective_id = if let Some(active) = self.active.remove(&raw_id) {
                    active.effective_id
                } else {
                    self.allocate(&raw_id)
                };
                self.latest_effective.insert(raw_id, effective_id.clone());
                tool_call.tool_call_id = effective_id;
            }
            LanguageModelV4StreamPart::ToolApprovalRequest(request) => {
                if let Some(effective_id) = self.latest_effective.get(&request.tool_call_id) {
                    request.tool_call_id = effective_id.clone();
                }
            }
            LanguageModelV4StreamPart::ToolResult(result) => {
                if let Some(effective_id) = self.latest_effective.get(&result.tool_call_id) {
                    result.tool_call_id = effective_id.clone();
                }
            }
            LanguageModelV4StreamPart::StreamStart { .. }
            | LanguageModelV4StreamPart::TextStart { .. }
            | LanguageModelV4StreamPart::TextDelta { .. }
            | LanguageModelV4StreamPart::TextEnd { .. }
            | LanguageModelV4StreamPart::ReasoningStart { .. }
            | LanguageModelV4StreamPart::ReasoningDelta { .. }
            | LanguageModelV4StreamPart::ReasoningEnd { .. }
            | LanguageModelV4StreamPart::ReasoningFile(_)
            | LanguageModelV4StreamPart::File(_)
            | LanguageModelV4StreamPart::Source(_)
            | LanguageModelV4StreamPart::Finish { .. }
            | LanguageModelV4StreamPart::Error { .. }
            | LanguageModelV4StreamPart::ResponseMetadata(_)
            | LanguageModelV4StreamPart::Raw { .. }
            | LanguageModelV4StreamPart::Custom { .. } => {}
        }
        Ok(())
    }

    fn allocate(&mut self, raw_id: &str) -> String {
        if self.used_effective.insert(raw_id.to_string()) {
            self.next_suffix.insert(raw_id.to_string(), 2);
            return raw_id.to_string();
        }

        let suffix = self.next_suffix.entry(raw_id.to_string()).or_insert(2);
        loop {
            let candidate = format!("{raw_id}_d{}", *suffix);
            *suffix += 1;
            if self.used_effective.insert(candidate.clone()) {
                return candidate;
            }
        }
    }
}
