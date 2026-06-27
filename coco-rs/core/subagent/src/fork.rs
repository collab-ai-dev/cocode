//! Fork-subagent pure logic — context cloning, XML rules, recursion guard.
//!
//! The runner (`root/coordinator`) reads these helpers; this crate only owns
//! the rules and the message shape, in line with the pure-logic charter
//! (no tokio, no QueryEngine, no AppState).
//!
//! When the FORK_SUBAGENT feature is enabled and no explicit `subagent_type`
//! is specified, the agent inherits the parent's byte-identical system prompt
//! (enabling prompt cache sharing) and receives the parent's conversation
//! history **verbatim** (real tool results preserved — see below), with a
//! fresh `<fork-boilerplate>` user turn appended via [`build_fork_child_message`].
//!
//! ## Why the parent history is passed through unmodified
//!
//! coco threads the parent's *pre-response* `ctx.messages` snapshot into the
//! fork — every `tool_use` already has its matching `tool_result`, and the
//! in-flight assistant turn (the one carrying the `Agent` call) is excluded.
//! Keeping the real results means the fork's request prefix is byte-identical
//! to the parent's, so the fork hits the parent's warm prompt cache AND sees
//! the file/command output the parent gathered. (An earlier version blanked
//! every `tool_result` to a placeholder — that diverged the prefix from the
//! parent, busting the cache it claimed to optimize, and blinded the child to
//! the parent's work. TS keeps real results and blanks only the in-flight
//! turn; coco has no in-flight turn to blank, so pass-through matches TS.)

use std::sync::Arc;

use coco_llm_types::LlmMessage;
use coco_llm_types::UserContentPart;
use coco_types::messages::Message;

/// XML tag wrapping the fork boilerplate rules.
///
/// Used for: (1) wrapping rules, (2) detecting recursive forks via tag scan.
pub const FORK_BOILERPLATE_TAG: &str = "fork-boilerplate";

/// Prefix before the directive text in fork child messages.
///
/// Note the trailing SPACE — the directive text is appended inline,
/// not on a new line.
pub const FORK_DIRECTIVE_PREFIX: &str = "Your directive: ";

/// Build the full child message with XML-wrapped rules + directive.
///
/// Produces a string ending with `</fork-boilerplate>\n\n{prefix}{directive}`
/// and no trailing newline.
pub fn build_fork_child_message(directive: &str) -> String {
    let rules = build_fork_child_rules();
    format!(
        "<{FORK_BOILERPLATE_TAG}>\n{rules}\n</{FORK_BOILERPLATE_TAG}>\n\n{FORK_DIRECTIVE_PREFIX}{directive}"
    )
}

/// Build the child rules body injected between the fork-boilerplate tags.
pub fn build_fork_child_rules() -> String {
    concat!(
        "STOP. READ THIS FIRST.\n",
        "\n",
        "You are a forked worker process. You are NOT the main agent.\n",
        "\n",
        "RULES (non-negotiable):\n",
        // Rule 1 uses U+2014 em-dash (the `—` escape in the original).
        "1. Your system prompt says \"default to forking.\" IGNORE IT \u{2014} that's for the parent. You ARE the fork. Do NOT spawn sub-agents; execute directly.\n",
        "2. Do NOT converse, ask questions, or suggest next steps\n",
        "3. Do NOT editorialize or add meta-commentary\n",
        "4. USE your tools directly: Bash, Read, Write, etc.\n",
        "5. If you modify files, commit your changes before reporting. Include the commit hash in your report.\n",
        "6. Do NOT emit text between tool calls. Use tools silently, then report once at the end.\n",
        // Rule 7 uses inline em-dash U+2014.
        "7. Stay strictly within your directive's scope. If you discover related systems outside your scope, mention them in one sentence at most \u{2014} other workers cover those areas.\n",
        "8. Keep your report under 500 words unless the directive specifies otherwise. Be factual and concise.\n",
        "9. Your response MUST begin with \"Scope:\". No preamble, no thinking-out-loud.\n",
        "10. REPORT structured facts, then stop\n",
        "\n",
        "Output format (plain text labels, not markdown headers):\n",
        "  Scope: <echo back your assigned scope in one sentence>\n",
        "  Result: <the answer or key findings, limited to the scope above>\n",
        // Output lines use inline em-dashes U+2014.
        "  Key files: <relevant file paths \u{2014} include for research tasks>\n",
        "  Files changed: <list with commit hash \u{2014} include only if you modified files>\n",
        "  Issues: <list \u{2014} include only if there are issues to flag>",
    )
    .to_string()
}

/// Check if we are inside a fork child (prevents recursive forking).
///
/// Scans user-role messages for the [`FORK_BOILERPLATE_TAG`] inside
/// any text content part — the tag is only present in fork child
/// contexts (injected by [`build_fork_child_message`]).
///
/// **Defense-in-depth (why the compaction gap is unreachable by default).**
/// This is a *message-scan* guard: it only detects a fork child while the
/// `<fork-boilerplate>` user turn is still in live history. The TS reference
/// pairs it with a PRIMARY, history-independent signal
/// (`querySource === 'agent:builtin:fork'`) that survives autocompaction;
/// coco-rs has no fork-source field on `ToolUseContext`. On its own, then, a
/// long-running fork that compacts away the boilerplate turn could re-enter
/// the fork path — a fork-of-fork.
///
/// In practice the recursion is blocked by a second, history-independent
/// layer: [`crate::filter::ALL_AGENT_DISALLOWED_TOOLS`] denies
/// [`coco_types::ToolName::Agent`] to *every* spawned subagent, forks
/// included (the coordinator spawn path merges
/// [`crate::filter::subagent_disallowed_tools`] into the child `ToolFilter`
/// unconditionally — only the async clamp skips forks). A fork child therefore
/// has no Agent tool to call, so it cannot reach `AgentTool::execute` to fork
/// again regardless of what compaction did to its history. The message-scan
/// guard becomes load-bearing only in ant builds that re-admit `Agent` for
/// nested-agent recursion (see `filter.rs`); closing the gap *there* needs a
/// typed fork marker on `ToolUseContext` (engine seam), tracked as a
/// follow-up. The whole fork feature is additionally gated behind the
/// default-off `COCO_FORK_SUBAGENT`.
pub fn is_in_fork_child(messages: &[Arc<Message>]) -> bool {
    let tag_marker = format!("<{FORK_BOILERPLATE_TAG}>");
    messages.iter().any(|arc| {
        let Message::User(user) = arc.as_ref() else {
            return false;
        };
        let LlmMessage::User { content, .. } = &user.message else {
            return false;
        };
        content.iter().any(|part| {
            matches!(
                part,
                UserContentPart::Text(t) if t.text.contains(&tag_marker)
            )
        })
    })
}

/// Check if fork subagent feature is enabled.
///
/// Enabled when [`coco_config::EnvKey::CocoForkSubagent`] (`COCO_FORK_SUBAGENT`)
/// is truthy (`1`/`true`/`yes`/`on`). **Note**: additional short-circuit to
/// `false` when coordinator mode is on or the session is non-interactive — that
/// gating happens in [`crate::coordinator_mode::is_fork_subagent_active`],
/// which composes this check with the coordinator/interactivity guards.
pub fn is_fork_enabled() -> bool {
    coco_config::env::is_env_truthy(coco_config::EnvKey::CocoForkSubagent)
}

#[cfg(test)]
#[path = "fork.test.rs"]
mod tests;
