//! Subagent handoff safety classifier — pure-logic prompt builders.
//!
//! `tools/AgentTool/agentToolUtils.ts:classifyHandoffIfNeeded` runs a
//! 2-stage LLM classifier on the subagent's transcript before returning
//! the result to the parent. Stage 1 (broad triage) flags anything
//! suspicious; stage 2 (focused review) confirms or clears the flag.
//! Only `auto` permission mode triggers classification — `default` /
//! `acceptEdits` already require user confirmation upstream and
//! `bypassPermissions` opts out.
//!
//! Bodies here build the prompts and parse the responses; the actual
//! LLM call lives in the runtime layer (the runner invokes a
//! [`coco_tool_runtime::SideQueryHandle`] with these prompts). Wiring
//! is tracked separately — `coco-coordinator::agent_handle` is the
//! intended call site after each subagent terminates with non-zero
//! tool uses.

/// Verdict from the safety classifier.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum HandoffClassification {
    /// Safe to return the subagent's output to the parent.
    Safe,
    /// One or more concerning actions detected. The reason is suitable
    /// for inclusion in a `<tool_use_error>` block surfaced to the
    /// parent model.
    Blocked { reason: String },
}

/// Whether classification should run for this hand-off. Gates *solely*
/// on a non-empty transcript — mirrors
/// `agentToolUtils.ts:411-412` (`const agentTranscript =
/// buildTranscriptForClassifier(...); if (!agentTranscript) return null`).
/// Read-only agents and zero-tool turns are NOT exempt; `subagentType`
/// and `totalToolUseCount` feed analytics only. The feature + auto-mode
/// gate is [`handoff_classifier_active`].
pub fn should_classify(transcript: &str) -> bool {
    !transcript.trim().is_empty()
}

/// Hand-off review user message — fed verbatim to the classifier so
/// it knows the transcript represents a sub-agent returning control to
/// the main agent (not a fresh tool call). Pulled from
/// `tools/AgentTool/agentToolUtils.ts:417`. Keep byte-faithful — the
/// classifier's training surface anchors on this exact phrasing.
pub const HANDOFF_REVIEW_USER_PROMPT: &str = "Sub-agent has finished and is handing back control to the main agent. \
     Review the sub-agent's work based on the block rules and let the main \
     agent know if any file is dangerous (the main agent will see the reason).";

/// Shared classifier policy — risk taxonomy, untrusted-evidence handling,
/// and the `SAFE` / `BLOCKED: <reason>` verdict contract. Kept in a
/// dedicated markdown file (rather than a Rust string literal) so prompt
/// changes are reviewable as a diff in isolation — the same auditability
/// pattern OpenAI's Codex CLI uses for its `guardian/policy.md`. The
/// YOLO classifier's own block-rules `.txt` is not shipped in the source
/// tree, so this is coco-rs's first-party policy; the user-message
/// framing ([`HANDOFF_REVIEW_USER_PROMPT`]) is byte-faithful to the
/// original.
pub const HANDOFF_CLASSIFIER_POLICY: &str = include_str!("handoff_classifier_policy.md");

/// Stage 1 system + user prompts. The model is asked to triage the
/// sub-agent transcript using the hand-off review framing and respond
/// with `SAFE` or `BLOCKED: <reason>`.
///
/// The system message is the shared [`HANDOFF_CLASSIFIER_POLICY`] plus a
/// one-line first-pass framing. The user message uses
/// [`HANDOFF_REVIEW_USER_PROMPT`] verbatim, paired with the agent type,
/// tool-use count, and the transcript so a stateless classifier has the
/// same context the YOLO flow gets through `agentMessages`.
pub fn stage1_prompts(
    agent_type: &str,
    transcript: &str,
    total_tool_use_count: i64,
) -> (String, String) {
    let system = format!(
        "{HANDOFF_CLASSIFIER_POLICY}\n\
         This is a first-pass triage of the hand-off: flag anything that warrants a closer look.",
    );
    let user = format!(
        "{HANDOFF_REVIEW_USER_PROMPT}\n\n\
         Sub-agent type: {agent_type}\n\
         Tool uses: {total_tool_use_count}\n\n\
         Transcript:\n{transcript}\n\n\
         Respond with `SAFE` if no concerns, or `BLOCKED: <reason>` if there are concerns.",
    );
    (system, user)
}

/// Stage 2 system + user prompts. Only invoked when stage 1 raised a
/// flag — confirms whether the concern is genuine or a false positive.
pub fn stage2_prompts(stage1_verdict: &str, transcript: &str) -> (String, String) {
    let system = format!(
        "{HANDOFF_CLASSIFIER_POLICY}\n\
         This is a second-stage review of a hand-off a first pass flagged. Confirm only \
         genuine concerns; if the earlier flag looks like a false positive, respond `SAFE`.",
    );
    let user = format!(
        "{HANDOFF_REVIEW_USER_PROMPT}\n\n\
         A previous review flagged this hand-off with: {stage1_verdict}\n\n\
         Re-examine the transcript and confirm if this is a genuine safety concern that \
         should block the hand-off, or a false positive.\n\n\
         Transcript:\n{transcript}\n\n\
         Respond with `SAFE` if the concern is a false positive, or `BLOCKED: <reason>` if \
         confirmed.",
    );
    (system, user)
}

/// Parse a classifier response (`SAFE` / `BLOCKED: ...`) into a verdict.
/// Lenient: leading whitespace and case variants of the keyword are
/// accepted; anything that doesn't start with the safe marker counts
/// as `Blocked`.
pub fn parse_classifier_response(text: &str) -> HandoffClassification {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        // Fail-open on empty responses — classifier failures should
        // never block legitimate output.
        return HandoffClassification::Safe;
    }
    let upper = trimmed.to_ascii_uppercase();
    if upper.starts_with("SAFE") {
        return HandoffClassification::Safe;
    }
    let reason = trimmed
        .strip_prefix("BLOCKED:")
        .or_else(|| trimmed.strip_prefix("BLOCKED"))
        .unwrap_or(trimmed)
        .trim()
        .to_string();
    HandoffClassification::Blocked { reason }
}

/// Compose the warning payload for a confirmed block.
/// Mirrors the literal returned by `classifyHandoffIfNeeded` in
/// `agentToolUtils.ts:476` — keep the wording byte-faithful so model
/// behaviour around the warning stays consistent. Returns `None` for
/// safe verdicts (caller passes the sub-agent's output through unchanged).
///
/// Empty reasons (e.g. classifier returned just `"BLOCKED"` with no
/// detail) collapse to `"unspecified safety concern"` so the message
/// never ends on a dangling colon.
pub fn render_block_message(verdict: &HandoffClassification) -> Option<String> {
    match verdict {
        HandoffClassification::Safe => None,
        HandoffClassification::Blocked { reason } => {
            let reason = if reason.is_empty() {
                "unspecified safety concern"
            } else {
                reason.as_str()
            };
            Some(format!(
                "SECURITY WARNING: This sub-agent performed actions that may violate security \
                 policy. Reason: {reason}. Review the sub-agent's actions carefully before \
                 acting on its output."
            ))
        }
    }
}

/// Warning text returned when the classifier itself was unreachable.
/// `agentToolUtils.ts:469`. Surfaces as a model-visible hint so the
/// parent agent knows the hand-off review didn't run, but still
/// propagates the sub-agent's output (fail-open).
pub const UNAVAILABLE_WARNING: &str = "Note: The safety classifier was unavailable when reviewing this sub-agent's work. \
     Please carefully verify the sub-agent's actions and output before acting on them.";

/// `agentToolUtils.ts:404-405` — handoff classification only runs
/// when the parent's permission mode is [`PermissionMode::Auto`] AND the
/// `TRANSCRIPT_CLASSIFIER` feature is on. Coco-rs surfaces the same
/// gate as a pure predicate so callers don't re-derive it. The
/// `feature_enabled` flag captures the feature-flag layer
/// (`feature('TRANSCRIPT_CLASSIFIER')` ≈ coco-rs runtime config; coco-rs
/// ships no such kill-switch yet, so callers pass `true`).
pub fn handoff_classifier_active(
    permission_mode: Option<coco_types::PermissionMode>,
    feature_enabled: bool,
) -> bool {
    feature_enabled && permission_mode == Some(coco_types::PermissionMode::Auto)
}

/// Build a transcript summary from agent messages for classification.
/// Strips tool-result bodies (only emits `tool_result` markers) so the
/// classifier sees actions, not data, and the prompt stays bounded.
///
/// `agentToolUtils.ts:buildTranscriptForClassifier` reads
/// `tool_result` blocks out of a user message's content array; coco-rs
/// stores them as the [`coco_types::messages::Message::ToolResult`]
/// variant but tags them `[user]` in the summary so the prompt the
/// classifier sees is byte-identical.
pub fn build_transcript_summary(
    messages: &[std::sync::Arc<coco_types::messages::Message>],
) -> String {
    use coco_llm_types::AssistantContentPart;
    use coco_llm_types::LlmMessage;
    use coco_llm_types::UserContentPart;
    use coco_types::messages::Message;

    let mut summary = String::new();
    for arc in messages {
        match arc.as_ref() {
            Message::User(u) => {
                if let LlmMessage::User { content, .. } = &u.message {
                    for part in content {
                        if let UserContentPart::Text(t) = part {
                            summary.push_str(&format!("[user] {}\n", t.text));
                        }
                    }
                }
            }
            Message::Assistant(a) => {
                if let LlmMessage::Assistant { content, .. } = &a.message {
                    for part in content {
                        match part {
                            AssistantContentPart::Text(t) => {
                                summary.push_str(&format!("[assistant] {}\n", t.text));
                            }
                            AssistantContentPart::ToolCall(tc) => {
                                summary
                                    .push_str(&format!("[assistant] tool_use: {}\n", tc.tool_name));
                            }
                            _ => {}
                        }
                    }
                }
            }
            Message::ToolResult(_) => {
                summary.push_str("[user] tool_result\n");
            }
            _ => {}
        }
    }
    summary
}

#[cfg(test)]
#[path = "handoff.test.rs"]
mod tests;
