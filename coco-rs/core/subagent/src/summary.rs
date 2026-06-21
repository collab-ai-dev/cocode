//! Subagent transcript summarisation — pure-logic prompt builders (E3).
//!
//! `services/AgentSummary/agentSummary.ts::buildSummaryPrompt`.
//!
//! This runs *periodically* (every 30 s) during execution to populate
//! a live `AgentProgress.summary` field shown in the coordinator UI.
//! `coco_coordinator::agent_handle::spawn` drives a 30 s timer that,
//! each tick, reads the child engine's live message history (via
//! [`coco_tool_runtime::LiveTranscript`]), skips when the transcript is
//! too short ([`should_summarize`], `transcript.messages.length < 3`),
//! cleans it with [`crate::filter_transcript`]
//! (`filterIncompleteToolCalls`), renders a bounded transcript with
//! [`render_transcript_tail`], and forks the summary call. Read-only
//! agent types are NOT exempt.
//!
//! Bodies build the prompts; the LLM call is driven by
//! [`coco_tool_runtime::SideQuery`] in the runtime layer
//! (`coco_coordinator::agent_handle`), which already wires the same
//! primitive for [`crate::handoff`].

/// Build the system + user prompts for a one-shot summary call.
///
/// Returns `(system_prompt, user_prompt)`. The system prompt is empty
/// — the entire instruction is folded into the user message, which
/// preserves byte-faithful cache parity with the parent's first turn.
///
/// `previous` is the prior summary (`None` on first call). Including
/// it in the prompt asks the model to say something *new* rather than
/// restate the previous summary — used for the periodic version; in
/// coco-rs we only call once but keep the field for forward
/// compatibility when the periodic mode lands.
///
/// **Byte-faithful with** `services/AgentSummary/agentSummary.ts::buildSummaryPrompt`
/// — including the empty-line separation around `prev_line`. Use a
/// concatenated raw-block layout instead of indented `\` continuations
/// so leading-whitespace from rustfmt never sneaks into the prompt
/// body (each continuation line in `format!("…\n\
///      indented")` keeps the indent literal in the output, breaking
/// cache parity).
pub fn build_summary_prompts(_agent_type: &str, previous: Option<&str>) -> (String, String) {
    let prev_line = match previous {
        Some(p) if !p.is_empty() => {
            format!("\nPrevious: \"{p}\" \u{2014} say something NEW.\n")
        }
        _ => String::new(),
    };

    let user = format!(
        "Describe your most recent action in 3-5 words using present tense (-ing). \
Name the file or function, not the branch. Do not use tools.
{prev_line}
Good: \"Reading runAgent.ts\"
Good: \"Fixing null check in validate.ts\"
Good: \"Running auth module tests\"
Good: \"Adding retry logic to fetchUser\"

Bad (past tense): \"Analyzed the branch diff\"
Bad (too vague): \"Investigating the issue\"
Bad (too long): \"Reviewing full branch diff and AgentTool.tsx integration\"
Bad (branch name): \"Analyzed adam/background-summary branch diff\""
    );

    (String::new(), user)
}

/// Minimum cleaned-transcript length for the periodic summary to run.
/// `agentSummary.ts` skips a tick when `transcript.messages.length < 3`:
/// fewer than three messages means only the bootstrap prompt (and maybe one
/// reply) exists, which isn't worth a summarizer fork yet.
pub const MIN_SUMMARY_MESSAGES: usize = 3;

/// Whether the periodic summary should run for a transcript of this size.
/// See [`MIN_SUMMARY_MESSAGES`].
pub fn should_summarize(message_count: usize) -> bool {
    message_count >= MIN_SUMMARY_MESSAGES
}

/// Render a (cleaned) message slice into the bounded transcript text fed to
/// the periodic summarizer fork.
///
/// Emits one `[role] …` line per part — assistant text, `tool_use: <name>`
/// markers, user text, and bare `tool_result` markers (bodies omitted: the
/// summarizer needs to know *which* actions ran, not their payloads). Only
/// the trailing `max_chars` are kept, snapped to a UTF-8 boundary, so a long
/// run can't blow the fork's input budget. The tail (not the head) is kept
/// because the summary describes the agent's *most recent* action.
pub fn render_transcript_tail(
    messages: &[std::sync::Arc<coco_types::messages::Message>],
    max_chars: usize,
) -> String {
    use coco_llm_types::AssistantContentPart;
    use coco_llm_types::LlmMessage;
    use coco_llm_types::UserContentPart;
    use coco_types::messages::Message;

    let mut out = String::new();
    for arc in messages {
        match arc.as_ref() {
            Message::User(u) => {
                if let LlmMessage::User { content, .. } = &u.message {
                    for part in content {
                        if let UserContentPart::Text(t) = part {
                            out.push_str(&format!("[user] {}\n", t.text));
                        }
                    }
                }
            }
            Message::Assistant(a) => {
                if let LlmMessage::Assistant { content, .. } = &a.message {
                    for part in content {
                        match part {
                            AssistantContentPart::Text(t) => {
                                out.push_str(&format!("[assistant] {}\n", t.text));
                            }
                            AssistantContentPart::ToolCall(tc) => {
                                out.push_str(&format!("[assistant] tool_use: {}\n", tc.tool_name));
                            }
                            _ => {}
                        }
                    }
                }
            }
            Message::ToolResult(_) => out.push_str("[user] tool_result\n"),
            _ => {}
        }
    }

    if out.len() > max_chars {
        // Snap to the char boundary at or *before* `len - max_chars` so the
        // suffix stays valid UTF-8 while keeping at least `max_chars` bytes.
        // Scanning backward (rather than forward) never drops a straddling
        // multibyte char and can't collapse to an empty string when the cap
        // is smaller than the trailing char's width; `0` is always a
        // boundary, so the loop terminates.
        let mut start = out.len() - max_chars;
        while !out.is_char_boundary(start) {
            start -= 1;
        }
        out = out[start..].to_string();
    }
    out
}

/// Filter the model's reply down to a clean summary string. Trims
/// whitespace, strips surrounding quotes, and rejects responses that
/// are empty or longer than 80 characters (defensive cap to keep the
/// panel from word-wrapping into multi-line). Returns `None` when the
/// model declined or produced noise.
pub fn sanitize_summary(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_matches('"').trim().to_string();
    if trimmed.is_empty() || trimmed.len() > 80 || trimmed.eq_ignore_ascii_case("none") {
        return None;
    }
    Some(trimmed)
}

#[cfg(test)]
#[path = "summary.test.rs"]
mod tests;
