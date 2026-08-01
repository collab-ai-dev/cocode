use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
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
    provider_references: HashMap<String, VecDeque<ProviderCallReference>>,
    next_suffix: HashMap<String, i64>,
    used_effective: HashSet<String>,
}

struct ActiveToolCall {
    effective_id: String,
    tool_name: String,
}

struct ProviderCallReference {
    effective_id: String,
    tool_name: String,
    approval_seen: bool,
    result_complete: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ToolCallIdError {
    Overlapping {
        raw_id: String,
        active_tool_name: String,
        incoming_tool_name: String,
    },
    UnmatchedReference {
        raw_id: String,
        reference_kind: &'static str,
    },
}

impl fmt::Display for ToolCallIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Overlapping {
                raw_id,
                active_tool_name,
                incoming_tool_name,
            } => write!(
                f,
                "provider emitted overlapping tool calls with id {raw_id:?}: active tool {active_tool_name:?}, incoming tool {incoming_tool_name:?}"
            ),
            Self::UnmatchedReference {
                raw_id,
                reference_kind,
            } => write!(
                f,
                "provider emitted {reference_kind} for reused tool call id {raw_id:?} without an unambiguous matching call"
            ),
        }
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
                    return Err(ToolCallIdError::Overlapping {
                        raw_id,
                        active_tool_name: active.tool_name.clone(),
                        incoming_tool_name: tool_name.clone(),
                    });
                }

                let effective_id = self.allocate(&raw_id);
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
                if tool_call.provider_executed == Some(true) {
                    self.provider_references
                        .entry(raw_id)
                        .or_default()
                        .push_back(ProviderCallReference {
                            effective_id: effective_id.clone(),
                            tool_name: tool_call.tool_name.clone(),
                            approval_seen: false,
                            result_complete: false,
                        });
                }
                tool_call.tool_call_id = effective_id;
            }
            LanguageModelV4StreamPart::ToolApprovalRequest(request) => {
                let raw_id = request.tool_call_id.clone();
                if let Some(references) = self.provider_references.get_mut(&raw_id) {
                    let reference = references
                        .iter_mut()
                        .find(|reference| !reference.approval_seen)
                        .ok_or(ToolCallIdError::UnmatchedReference {
                            raw_id,
                            reference_kind: "tool approval request",
                        })?;
                    reference.approval_seen = true;
                    request.tool_call_id = reference.effective_id.clone();
                }
            }
            LanguageModelV4StreamPart::ToolResult(result) => {
                let raw_id = result.tool_call_id.clone();
                if let Some(references) = self.provider_references.get_mut(&raw_id) {
                    let reference_index = references
                        .iter()
                        .position(|reference| {
                            !reference.result_complete && reference.tool_name == result.tool_name
                        })
                        .or_else(|| {
                            references
                                .iter()
                                .position(|reference| !reference.result_complete)
                        })
                        .ok_or(ToolCallIdError::UnmatchedReference {
                            raw_id,
                            reference_kind: "tool result",
                        })?;
                    let reference = &mut references[reference_index];
                    if result.preliminary != Some(true) {
                        reference.result_complete = true;
                    }
                    result.tool_call_id = reference.effective_id.clone();
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

#[cfg(test)]
#[path = "tool_call_id.test.rs"]
mod tests;
