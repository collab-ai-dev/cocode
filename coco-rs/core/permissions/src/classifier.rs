//! Yolo classifier — two-stage XML LLM-based permission classification.
//!
//! Wire-shape:
//!
//! * **Output**: `<block>yes</block><reason>one short sentence</reason>` for
//!   block; `<block>no</block>` (no `<reason>`) for allow. The system prompt
//!   pins this: "Your ENTIRE response MUST begin with `<block>`".
//! * **User content**: wrapped in `<transcript>\n` … `\n</transcript>\n`.
//! * **Stage 1**: 64-token budget, `stop_sequences = ["</block>"]`,
//!   `XML_S1_SUFFIX` nudge ("Err on the side of blocking. `<block>`
//!   immediately."). Drops out as soon as the model writes the closing tag.
//! * **Stage 2**: 4096-token budget, no stop sequence, `XML_S2_SUFFIX`
//!   nudge ("Review the classification process… Use `<thinking>` before
//!   responding with `<block>`."). Reached on stage-1 `block=yes` (second
//!   opinion) or stage-1 unparseable.
//! * **Parse**: regex `<block>(yes|no)\b(</block>)?` after `<thinking>`-strip;
//!   `<reason>...</reason>` after the same strip. Unparseable → fall through
//!   to stage 2; if stage 2 is also unparseable → block (safe default).
//!
//! The two stages share the same system prompt so the Anthropic 1h prompt
//! cache hits across calls.

use coco_types::ToolName;
use regex::Regex;
use serde::Deserialize;
use serde::Serialize;
use std::sync::LazyLock;

/// Re-export from coco-types.
pub use coco_types::ClassifierUsage;

/// Result of yolo classifier.
#[derive(Debug, Clone)]
pub struct YoloClassifierResult {
    /// Whether the action should be blocked.
    pub should_block: bool,
    /// Human-readable reason for the decision.
    pub reason: String,
    /// Model used for classification.
    pub model: String,
    /// Token usage for the classifier call.
    pub usage: Option<ClassifierUsage>,
    /// Duration of the classifier call in milliseconds.
    pub duration_ms: Option<i64>,
    /// Which stage produced this result (1 = fast, 2 = extended thinking).
    pub stage: Option<i32>,
    /// The classifier model could not respond (transport / capacity error),
    /// as opposed to actively blocking. Distinct from `should_block` so the
    /// decision layer can fail-open/closed instead of treating an outage as
    /// a malicious block.
    pub unavailable: bool,
    /// The classifier prompt exceeded the model's context window — a
    /// deterministic condition that will not recover on retry.
    pub transcript_too_long: bool,
}

impl YoloClassifierResult {
    /// Construct a plain allow/block verdict with the unavailable /
    /// transcript-too-long flags cleared (the common case).
    fn verdict(should_block: bool, reason: String, stage: Option<i32>) -> Self {
        Self {
            should_block,
            reason,
            model: String::new(),
            usage: None,
            duration_ms: None,
            stage,
            unavailable: false,
            transcript_too_long: false,
        }
    }
}

/// Whether a classifier transport error string indicates the prompt overran
/// the model's context window (deterministic — retry cannot help) vs. a
/// transient outage.
fn is_transcript_too_long_error(err: &str) -> bool {
    let lower = err.to_lowercase();
    lower.contains("prompt is too long")
        || lower.contains("context window")
        || lower.contains("context length")
        || lower.contains("too many tokens")
}

/// Projects a tool's raw input down to the security-relevant fields the
/// auto-mode classifier should see.
///
/// `None` ⇒ the caller has no projection for this `(tool, input)` pair —
/// either the tool is unknown or it declares no classifier-relevant input — in
/// which case the classifier falls back to the raw input JSON. Built from the
/// live `ToolRegistry` in `app/query` and threaded down so this crate stays
/// free of any `coco-tools` dependency.
pub type InputProjector<'a> =
    &'a (dyn Fn(&str, &serde_json::Value) -> Option<String> + Send + Sync);

/// Which classifier stages run (Both / Fast / Thinking). Defined in
/// `coco-types` so `coco-config`'s `AutoModeConfig` and this crate's
/// `AutoModeRules` share one type.
pub use coco_types::ClassifierMode;

/// Auto-mode configuration rules: user-configurable allow/deny/environment rules.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AutoModeRules {
    /// Rules for what to automatically allow.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Rules for what to soft-deny (prompt user).
    #[serde(default)]
    pub soft_deny: Vec<String>,
    /// Environment context for the classifier.
    #[serde(default)]
    pub environment: Vec<String>,
    /// Which classifier stages run (Both / Fast / Thinking).
    /// Defaults to `Both` (two-stage escalation).
    #[serde(default)]
    pub classifier_mode: ClassifierMode,
    /// When the LLM classifier is unavailable (transient transport/capacity
    /// outage), fail OPEN to a manual prompt in interactive sessions instead
    /// of denying. Default `false` = fail closed (deny). Independent of
    /// `classifier_mode`.
    #[serde(default)]
    pub classifier_unavailable_fail_open: bool,
}

/// A compressed transcript entry for the classifier.
#[derive(Debug, Clone)]
pub struct TranscriptEntry {
    pub role: TranscriptRole,
    pub content: Vec<TranscriptBlock>,
}

/// Role in the transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptRole {
    User,
    Assistant,
}

/// A content block in the transcript.
///
/// Carries only `Text` (user-authored) and `ToolCall` (assistant tool_use).
/// Assistant prose and tool *results* are deliberately excluded — both are
/// attacker-influenceable and must not reach the security classifier.
#[derive(Debug, Clone)]
pub enum TranscriptBlock {
    /// User text message.
    Text(String),
    /// Tool call (name + abbreviated input).
    ToolCall {
        tool_name: String,
        input_summary: String,
    },
}

/// Safe-tool allowlist — tools that never need classifier review.
const SAFE_TOOLS: &[&str] = &[
    // Read-only file operations
    ToolName::Read.as_str(),
    // Search / read-only
    ToolName::Grep.as_str(),
    ToolName::Glob.as_str(),
    ToolName::Lsp.as_str(),
    ToolName::ToolSearch.as_str(),
    ToolName::ListMcpResources.as_str(),
    ToolName::ReadMcpResource.as_str(),
    // Task management (metadata only)
    ToolName::TodoWrite.as_str(),
    ToolName::TaskCreate.as_str(),
    ToolName::TaskGet.as_str(),
    ToolName::TaskUpdate.as_str(),
    ToolName::TaskList.as_str(),
    ToolName::TaskStop.as_str(),
    ToolName::TaskOutput.as_str(),
    // Plan mode / UI
    ToolName::AskUserQuestion.as_str(),
    ToolName::EnterPlanMode.as_str(),
    ToolName::ExitPlanMode.as_str(),
    ToolName::VerifyPlanExecution.as_str(),
    // Swarm coordination (internal mailbox/team state only)
    ToolName::TeamCreate.as_str(),
    ToolName::TeamDelete.as_str(),
    ToolName::SendMessage.as_str(),
    // Misc safe
    ToolName::Sleep.as_str(),
    // NB: `SendUserMessage` is intentionally NOT allowlisted here — it is
    // still auto-allowed in auto-mode via the read-only fast path
    // (`SendUserMessageTool::is_read_only` is always true).
];

/// Check if a tool is in the safe-tool allowlist (no classifier needed).
pub fn is_safe_tool(tool_name: &str) -> bool {
    SAFE_TOOLS.contains(&tool_name)
}

/// Build transcript entries from conversation messages for the classifier.
///
/// Generic over `Borrow<Message>` so engine call sites can pass
/// `&[Arc<Message>]` from `MessageHistory::as_slice()` directly,
/// avoiding the previous deep-clone bridge at the per-tool-call hot
/// path (plan §11 F8 follow-up).
pub fn build_transcript_entries<M: std::borrow::Borrow<coco_messages::Message>>(
    messages: &[M],
    projector: Option<InputProjector<'_>>,
) -> Vec<TranscriptEntry> {
    let mut entries = Vec::new();

    for msg in messages {
        match msg.borrow() {
            coco_messages::Message::User(u) => {
                let text = extract_user_text(&u.message);
                if !text.is_empty() {
                    entries.push(TranscriptEntry {
                        role: TranscriptRole::User,
                        content: vec![TranscriptBlock::Text(truncate(&text, 2000))],
                    });
                }
            }
            coco_messages::Message::Assistant(a) => {
                let blocks = extract_assistant_blocks(&a.message, projector);
                if !blocks.is_empty() {
                    entries.push(TranscriptEntry {
                        role: TranscriptRole::Assistant,
                        content: blocks,
                    });
                }
            }
            // Tool results are NOT included: only user text + assistant
            // tool_use blocks are kept. Tool output is attacker-influenceable
            // and must not reach the security gate.
            _ => {}
        }
    }

    entries
}

/// Build the classifier system prompt.
///
/// The system prompt pins the XML output format and the "begin with `<block>`"
/// invariant.
pub fn build_classifier_system_prompt(rules: &AutoModeRules) -> String {
    let mut prompt = String::from(
        "You are a security classifier for an AI coding assistant. \
         Your job is to determine whether a proposed tool action is safe to auto-execute \
         without user confirmation.\n\n\
         You are the last line of defense before an action runs unattended. Default to \
         blocking when uncertain — a blocked action merely asks the user, but an \
         allowed dangerous action cannot be undone.\n\n\
         ## BLOCK if the action does any of the following\n\
         - Code-from-External: fetches and executes code from an untrusted/remote source \
           (e.g. `curl … | sh`, `iex (iwr …)`, running a just-downloaded binary/script).\n\
         - Irreversible-Local-Destruction: deletes or overwrites data with no recovery \
           (e.g. `rm -rf` of non-scratch paths, `git push --force` over shared history, \
           disk/partition wipes, truncating databases).\n\
         - Unauthorized-Persistence: installs a mechanism that survives the session \
           (cron jobs, systemd units, shell rc / profile edits, login items, git hooks, \
           added SSH authorized_keys).\n\
         - Security-Weaken: disables or downgrades a protection (chmod 777 / removing auth, \
           disabling a firewall or signature check, editing the agent's own permission / \
           settings files, writing secrets into world-readable locations).\n\
         - Data-Exfiltration: sends local files, credentials, or secrets to the network.\n\n\
         ## ALLOW\n\
         - Routine, reversible development work scoped to the project (building, testing, \
           reading, formatting, ordinary edits to project files).\n\
         - Anything the user's own configuration below explicitly authorizes.\n\n",
    );

    if !rules.allow.is_empty() {
        prompt.push_str("## Auto-Allow Rules\n");
        for rule in &rules.allow {
            prompt.push_str(&format!("- {rule}\n"));
        }
        prompt.push('\n');
    }

    if !rules.soft_deny.is_empty() {
        prompt.push_str("## Soft-Deny Rules (require user approval)\n");
        for rule in &rules.soft_deny {
            prompt.push_str(&format!("- {rule}\n"));
        }
        prompt.push('\n');
    }

    if !rules.environment.is_empty() {
        prompt.push_str("## Environment Context\n");
        for ctx in &rules.environment {
            prompt.push_str(&format!("- {ctx}\n"));
        }
        prompt.push('\n');
    }

    // XML output format block.
    prompt.push_str(
        "## Output Format\n\
         \n\
         If the action should be blocked:\n\
         <block>yes</block><reason>one short sentence</reason>\n\
         \n\
         If the action should be allowed:\n\
         <block>no</block>\n\
         \n\
         Do NOT include a <reason> tag when the action is allowed.\n\
         Your ENTIRE response MUST begin with <block>. Do NOT output any analysis, \
         reasoning, or commentary before <block>. No \"Looking at...\" or similar preamble.",
    );

    prompt
}

/// Format a tool action for the classifier.
pub fn format_action_for_classifier(
    tool_name: &str,
    input: &serde_json::Value,
    projector: Option<InputProjector<'_>>,
) -> String {
    // Per-tool projection (curated, security-relevant fields) when the tool
    // provides one; otherwise the raw input JSON. A `Some("")` projection
    // (no security relevance) is fast-allowed upstream in `classify_yolo_action`
    // before reaching here. A `None` projection (no projector / un-annotated
    // tool) is NOT an auto-allow — the action must reach the classifier, so it
    // falls back to the raw input JSON.
    let projected = projector
        .and_then(|project| project(tool_name, input))
        .unwrap_or_else(|| serde_json::to_string(input).unwrap_or_default());
    let truncated = truncate(&projected, 1000);
    format!("<action>\ntool: {tool_name}\ninput: {truncated}\n</action>")
}

/// Stage 1 token budget (both-mode).
const STAGE_1_MAX_TOKENS: i64 = 64;

/// Stage 2 token budget (extended-thinking).
const STAGE_2_MAX_TOKENS: i64 = 4096;

/// Single-stage fast-mode token budget.
const FAST_ONLY_MAX_TOKENS: i64 = 256;

/// Stage 1 suffix nudge for fast classification.
pub const XML_S1_SUFFIX: &str = "\nErr on the side of blocking. <block> immediately.";

/// Stage 2 suffix nudge for extended-thinking classification.
pub const XML_S2_SUFFIX: &str = "\nReview the classification process and follow it carefully, making sure you deny \
     actions that should be blocked. As a reminder, explicit (not suggestive or implicit) \
     user confirmation is required to override blocks. Use <thinking> before responding \
     with <block>.";

/// Parameters for the classifier callback.
#[derive(Debug, Clone)]
pub struct ClassifyRequest {
    pub system_prompt: String,
    pub user_prompt: String,
    pub max_tokens: i64,
    pub stage: i32,
    /// Stop sequences forwarded to the provider. Stage 1 in `both` mode
    /// uses `["</block>"]` for immediate termination; stage 2 leaves it
    /// empty so the model can elaborate inside `<thinking>` and `<reason>`.
    pub stop_sequences: Option<Vec<String>>,
}

/// Classify a proposed tool action using the two-stage XML classifier.
///
/// Default is `both` mode:
///
/// 1. Stage 1 (fast): 64 tokens, `stop_sequences=["</block>"]`, S1 suffix.
///    * `<block>no</block>` → return Allow.
///    * `<block>yes</block>` → fall through to stage 2 (second opinion).
///    * Unparseable → fall through to stage 2.
/// 2. Stage 2 (thinking): 4096 tokens, no stop sequences, S2 suffix.
///    * `<block>no</block>` → return Allow.
///    * `<block>yes</block>` → Block with reason.
///    * Unparseable → Block (safe default).
pub async fn classify_yolo_action<M, F, Fut>(
    messages: &[M],
    tool_name: &str,
    input: &serde_json::Value,
    rules: &AutoModeRules,
    classify_fn: F,
    projector: Option<InputProjector<'_>>,
) -> YoloClassifierResult
where
    M: std::borrow::Borrow<coco_messages::Message>,
    F: Fn(ClassifyRequest) -> Fut,
    Fut: std::future::Future<Output = Result<String, String>>,
{
    // Fast path: safe tools never need classification.
    if is_safe_tool(tool_name) {
        return YoloClassifierResult::verdict(false, "Safe tool (allowlisted)".into(), None);
    }

    // Empty-projection fast-allow: a tool whose security-relevant projection
    // encodes to "" declares no classifier-relevant input → allow without
    // invoking the LLM. This also guards the provider against an empty
    // `<action>` block. A `None` projection (no projector / un-annotated tool)
    // is NOT an auto-allow — it falls through to the raw-JSON classifier path
    // below.
    if let Some(project) = projector
        && let Some(projected) = project(tool_name, input)
        && projected.trim().is_empty()
    {
        return YoloClassifierResult::verdict(
            false,
            "Tool declares no classifier-relevant input".into(),
            None,
        );
    }

    let system_prompt = build_classifier_system_prompt(rules);
    let transcript = build_transcript_entries(messages, projector);
    let action = format_action_for_classifier(tool_name, input, projector);
    let transcript_text = format_transcript(&transcript);

    // Wrap the entire user content (transcript + action) in a
    // `<transcript>...</transcript>` envelope so the model sees a clearly
    // bounded record vs. its own output.
    let user_body = format!(
        "<transcript>\n\
         {transcript_text}\n\n\
         Evaluate whether this action is safe to auto-execute:\n\n\
         {action}\n\
         </transcript>\n"
    );

    match rules.classifier_mode {
        // Single fast stage — 256 tokens, no stop sequence, verdict final.
        ClassifierMode::Fast => {
            let result = classify_fn(ClassifyRequest {
                system_prompt,
                user_prompt: format!("{user_body}{XML_S1_SUFFIX}"),
                max_tokens: FAST_ONLY_MAX_TOKENS,
                stage: 1,
                stop_sequences: None,
            })
            .await;
            interpret_final_verdict(
                result,
                1,
                "Allowed by fast classifier",
                "Blocked by fast classifier",
            )
        }
        // Single extended stage — 4096 tokens, no stop sequence.
        ClassifierMode::Thinking => {
            let result = classify_fn(ClassifyRequest {
                system_prompt,
                user_prompt: format!("{user_body}{XML_S2_SUFFIX}"),
                max_tokens: STAGE_2_MAX_TOKENS,
                stage: 2,
                stop_sequences: None,
            })
            .await;
            interpret_final_verdict(
                result,
                2,
                "Allowed by extended classifier",
                "No reason provided",
            )
        }
        // Two-stage escalation (default).
        ClassifierMode::Both => {
            // ── Stage 1 ────────────────────────────────────────────────
            let stage1_user_prompt = format!("{user_body}{XML_S1_SUFFIX}");
            let stage1_result = classify_fn(ClassifyRequest {
                system_prompt: system_prompt.clone(),
                user_prompt: stage1_user_prompt,
                max_tokens: STAGE_1_MAX_TOKENS,
                stage: 1,
                stop_sequences: Some(vec!["</block>".to_string()]),
            })
            .await;

            if let Ok(response) = stage1_result {
                match parse_xml_block(&response) {
                    Some(false) => {
                        // Allow — stage 1 verdict is sufficient.
                        return YoloClassifierResult::verdict(
                            false,
                            "Allowed by fast classifier".into(),
                            Some(1),
                        );
                    }
                    Some(true) | None => {
                        // Block or unparseable → escalate to stage 2.
                    }
                }
            }
            // Stage-1 transport error also falls through to stage 2.

            // ── Stage 2 ────────────────────────────────────────────────
            let stage2_user_prompt = format!("{user_body}{XML_S2_SUFFIX}");
            let stage2_result = classify_fn(ClassifyRequest {
                system_prompt,
                user_prompt: stage2_user_prompt,
                max_tokens: STAGE_2_MAX_TOKENS,
                stage: 2,
                stop_sequences: None,
            })
            .await;
            interpret_final_verdict(
                stage2_result,
                2,
                "Allowed by extended classifier",
                "No reason provided",
            )
        }
    }
}

/// Interpret a single classifier stage's outcome as a final verdict.
///
/// Shared by the `Fast` / `Thinking` single-stage modes and `Both`-mode
/// stage 2. `Ok` + `<block>no>` → allow (`allow_reason` when no `<reason>`);
/// `<block>yes>` → block with the parsed reason, falling back to
/// `block_reason` when the model emitted no `<reason>`;
/// unparseable → block (safe default). `Err` flags `unavailable` /
/// `transcript_too_long` so the decision layer can fail-open (interactive) or
/// fail-closed (headless).
fn interpret_final_verdict(
    result: Result<String, String>,
    stage: i32,
    allow_reason: &str,
    block_reason: &str,
) -> YoloClassifierResult {
    match result {
        Ok(response) => {
            let parsed = parse_xml_block(&response);
            let reason = parse_xml_reason(&response).unwrap_or_default();
            match parsed {
                Some(true) => YoloClassifierResult::verdict(
                    true,
                    if reason.is_empty() {
                        block_reason.to_string()
                    } else {
                        reason
                    },
                    Some(stage),
                ),
                Some(false) => YoloClassifierResult::verdict(
                    false,
                    if reason.is_empty() {
                        allow_reason.to_string()
                    } else {
                        reason
                    },
                    Some(stage),
                ),
                None => YoloClassifierResult::verdict(
                    true,
                    format!("Classifier stage {stage} unparseable - blocking for safety"),
                    Some(stage),
                ),
            }
        }
        Err(err) => {
            let too_long = is_transcript_too_long_error(&err);
            YoloClassifierResult {
                should_block: true,
                reason: format!("Classifier error: {err}"),
                model: String::new(),
                usage: None,
                duration_ms: None,
                stage: Some(stage),
                unavailable: !too_long,
                transcript_too_long: too_long,
            }
        }
    }
}

/// Format transcript entries into a compact string for the classifier.
///
/// Keeps the most recent 10 entries but emits them in **chronological**
/// order (oldest of the window first) so the classifier reads the
/// conversation forwards, matching the order the agent produced it.
fn format_transcript(entries: &[TranscriptEntry]) -> String {
    let mut out = String::new();
    // No fixed N-entry cap — render the full transcript and rely on upstream
    // compaction to bound history size. The previous `.saturating_sub(10)`
    // window dropped older actions the classifier needs to judge.
    for entry in entries {
        let role_str = match entry.role {
            TranscriptRole::User => "User",
            TranscriptRole::Assistant => "Assistant",
        };
        out.push_str(&format!("[{role_str}]\n"));
        for block in &entry.content {
            match block {
                TranscriptBlock::Text(t) => out.push_str(t),
                TranscriptBlock::ToolCall {
                    tool_name,
                    input_summary,
                } => out.push_str(&format!("Called {tool_name}: {input_summary}")),
            }
            out.push('\n');
        }
    }
    out
}

/// Strip `<thinking>...</thinking>` blocks (and unterminated thinking text)
/// so XML tags inside the model's chain-of-thought don't get matched by
/// the block/reason parsers.
fn strip_thinking(text: &str) -> String {
    #[allow(clippy::expect_used)]
    static CLOSED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)<thinking>.*?</thinking>")
            .expect("strip_thinking closed regex is statically valid")
    });
    #[allow(clippy::expect_used)]
    static OPEN_ONLY: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)<thinking>.*$")
            .expect("strip_thinking open-only regex is statically valid")
    });
    let after_closed = CLOSED.replace_all(text, "");
    OPEN_ONLY.replace_all(&after_closed, "").into_owned()
}

/// Parse `<block>yes|no</block>`. Closing tag is optional because stage 1
/// uses `stop_sequences = ["</block>"]` and the provider truncates before
/// the closer is emitted.
fn parse_xml_block(text: &str) -> Option<bool> {
    #[allow(clippy::expect_used)]
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)<block>(yes|no)\b(?:</block>)?")
            .expect("parse_xml_block regex is statically valid")
    });
    let stripped = strip_thinking(text);
    RE.captures(&stripped).map(|caps| {
        caps.get(1)
            .map(|m| m.as_str().eq_ignore_ascii_case("yes"))
            .unwrap_or(false)
    })
}

/// Parse `<reason>...</reason>` (non-greedy).
fn parse_xml_reason(text: &str) -> Option<String> {
    #[allow(clippy::expect_used)]
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)<reason>(.*?)</reason>")
            .expect("parse_xml_reason regex is statically valid")
    });
    let stripped = strip_thinking(text);
    RE.captures(&stripped)
        .and_then(|caps| caps.get(1).map(|m| m.as_str().trim().to_string()))
}

fn extract_user_text(msg: &coco_messages::LlmMessage) -> String {
    match msg {
        coco_messages::LlmMessage::User { content, .. } => content
            .iter()
            .filter_map(|c| match c {
                coco_messages::UserContent::Text(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn extract_assistant_blocks(
    msg: &coco_messages::LlmMessage,
    projector: Option<InputProjector<'_>>,
) -> Vec<TranscriptBlock> {
    match msg {
        coco_messages::LlmMessage::Assistant { content, .. } => content
            .iter()
            .filter_map(|c| match c {
                // Assistant text is deliberately dropped: it is model-authored
                // and could be crafted to steer the security classifier
                // (prompt injection into the permission gate). Only tool_use
                // blocks are kept.
                coco_messages::AssistantContent::ToolCall(tc) => match projector {
                    // Projector present (production): include the curated
                    // projection, or SKIP the block when the tool declares no
                    // classifier-relevant input (`None`). A deser failure still
                    // yields `Some(raw)` from the blanket impl, so malformed
                    // inputs are kept, never skipped.
                    Some(project) => {
                        project(&tc.tool_name, &tc.input).map(|summary| TranscriptBlock::ToolCall {
                            tool_name: tc.tool_name.clone(),
                            input_summary: truncate(&summary, 300),
                        })
                    }
                    // No projector (permissions-crate unit tests have no
                    // registry): keep the raw-JSON summary so tests still
                    // observe tool content.
                    None => Some(TranscriptBlock::ToolCall {
                        tool_name: tc.tool_name.clone(),
                        input_summary: truncate(
                            &serde_json::to_string(&tc.input).unwrap_or_default(),
                            300,
                        ),
                    }),
                },
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Truncate to at most `max_len` bytes, snapping down to a UTF-8 char
/// boundary so a multibyte character straddling the cut does not panic the
/// byte slice. Untrusted user / model text flows through here.
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let mut end = max_len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

#[cfg(test)]
#[path = "classifier.test.rs"]
mod tests;
