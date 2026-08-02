use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt;

use vercel_ai_provider::LanguageModelV4StreamPart;

/// Assigns a stable, response-local identity to every tool call.
///
/// Some providers reuse their wire ID for multiple sequential calls. The
/// effective ID must be fixed before stream events and the final snapshot are
/// emitted so every downstream map observes the same identity.
///
/// Scope is one provider response. Duplicates that span *messages* (a provider
/// that restarts numbering every turn, so a replayed prompt carries the same id
/// N times) are outside this seam and are resolved at the pre-API chokepoint by
/// `coco_messages::normalize` — see that crate's `DedupToolCallIds` pass.
///
/// `ToolResult` and `ToolApprovalRequest` parts are deliberately **not**
/// rewritten: both are dropped downstream (`stream.rs` keeps neither in the
/// turn snapshot, and `app/query` does not reconstruct approval requests into
/// assistant history), so binding them to a renamed call would be bookkeeping
/// nobody reads. If either is ever round-tripped into assistant content, they
/// need a FIFO binding per reused raw id — results preferring an unfinished
/// call with the same tool name, preliminary results not closing a call — and
/// a mismatch must stay non-fatal.
#[derive(Default)]
pub(crate) struct ToolCallIdNormalizer {
    active: HashMap<String, ActiveToolCall>,
    next_suffix: HashMap<String, i64>,
    used_effective: HashSet<String>,
}

struct ActiveToolCall {
    effective_id: String,
    tool_name: String,
}

/// Overlapping reuse of a still-open tool-call id. Deltas for the two calls
/// are indistinguishable, so the stream cannot be attributed and is aborted.
///
/// This is the ONLY fatal condition in this module. Every other mismatch
/// (an approval or result that binds to no known call) degrades to leaving
/// the wire id untouched — see [`ToolCallIdNormalizer::normalize`].
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ToolCallIdError {
    raw_id: String,
    active_tool_name: String,
    incoming_tool_name: String,
}

impl fmt::Display for ToolCallIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            raw_id,
            active_tool_name,
            incoming_tool_name,
        } = self;
        write!(
            f,
            "provider emitted overlapping tool calls with id {raw_id:?}: active tool {active_tool_name:?}, incoming tool {incoming_tool_name:?}"
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
                tool_call.tool_call_id = effective_id;
            }
            // Left verbatim on purpose — see the type docs.
            LanguageModelV4StreamPart::ToolApprovalRequest(_)
            | LanguageModelV4StreamPart::ToolResult(_)
            | LanguageModelV4StreamPart::StreamStart { .. }
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
