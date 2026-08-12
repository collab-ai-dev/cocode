use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
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
/// Approval requests are persisted in assistant history, so they bind to the
/// effective call id. Completed provider-executed calls wait in FIFO order per
/// raw id; a request emitted before the canonical close binds to the active
/// call. A mismatch stays non-fatal and leaves the provider id unchanged.
/// Provider tool results retire the matching pending call. This prevents a
/// late approval for a reused raw id from binding to a call that already
/// completed without approval.
#[derive(Default)]
pub(crate) struct ToolCallIdNormalizer {
    active: HashMap<String, ActiveToolCall>,
    approval_bindings: HashMap<String, VecDeque<ProviderCallBinding>>,
    next_suffix: HashMap<String, i64>,
    used_effective: HashSet<String>,
}

struct ActiveToolCall {
    effective_id: String,
    tool_name: String,
    provider_executed: Option<bool>,
    approval_bound: bool,
}

struct ProviderCallBinding {
    effective_id: String,
    tool_name: String,
    approval_bound: bool,
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
            LanguageModelV4StreamPart::ToolInputStart {
                id,
                tool_name,
                provider_executed,
                ..
            } => {
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
                        provider_executed: *provider_executed,
                        approval_bound: false,
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
                let (effective_id, approval_bound, started_provider_executed) =
                    if let Some(active) = self.active.remove(&raw_id) {
                        (
                            active.effective_id,
                            active.approval_bound,
                            active.provider_executed,
                        )
                    } else {
                        (self.allocate(&raw_id), false, None)
                    };
                if tool_call.provider_executed.or(started_provider_executed) == Some(true) {
                    self.approval_bindings.entry(raw_id).or_default().push_back(
                        ProviderCallBinding {
                            effective_id: effective_id.clone(),
                            tool_name: tool_call.tool_name.clone(),
                            approval_bound,
                        },
                    );
                }
                tool_call.tool_call_id = effective_id;
            }
            LanguageModelV4StreamPart::ToolApprovalRequest(request) => {
                let raw_id = request.tool_call_id.clone();
                if let Some(active) = self.active.get_mut(&raw_id) {
                    request.tool_call_id = active.effective_id.clone();
                    active.approval_bound = true;
                } else if let Some(binding) =
                    self.approval_bindings
                        .get_mut(&raw_id)
                        .and_then(|bindings| {
                            bindings.iter_mut().find(|binding| !binding.approval_bound)
                        })
                {
                    request.tool_call_id = binding.effective_id.clone();
                    binding.approval_bound = true;
                }
            }
            LanguageModelV4StreamPart::ToolResult(result) => {
                let raw_id = result.tool_call_id.clone();
                let is_final = result.preliminary != Some(true);
                if let Some(bindings) = self.approval_bindings.get_mut(&raw_id)
                    && let Some(position) = bindings
                        .iter()
                        .position(|binding| binding.tool_name == result.tool_name)
                {
                    result.tool_call_id = bindings[position].effective_id.clone();
                    if is_final {
                        bindings.remove(position);
                    }
                }
                if self
                    .approval_bindings
                    .get(&raw_id)
                    .is_some_and(VecDeque::is_empty)
                {
                    self.approval_bindings.remove(&raw_id);
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
            | LanguageModelV4StreamPart::Custom(_) => {}
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
