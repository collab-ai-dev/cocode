//! Session title / name auto-generation.
//!
//! Two LLM-helper flows, sharing the `ModelRole::Fast` resolver but
//! producing different artifacts for different consumers:
//!
//! - **Title** (`build_title_prompt` / `parse_title_response` /
//!   `apply_title`): Sentence case, 3-7 words. Persists as
//!   `MetadataEntry::AiTitle` so a user `/rename` always wins on read.
//! - **Session name** (`build_session_name_prompt` /
//!   `parse_session_name_response`): Triggered by bare
//!   `/rename` and post-plan auto-name, kebab-case 2-4 words derived
//!   from caller-provided context. Persists as
//!   `MetadataEntry::CustomTitle` (it's user-initiated even though the
//!   LLM chose the wording).
//!
//! This module is prompt/parse-only — it does not own the LLM
//! invocation. Callers resolve the `ModelSpec` via `ModelRoles::get(Fast)`,
//! run the generated prompt through their inference layer, and pass the raw
//! response here for parsing.

use serde::Deserialize;

/// Minimum title length we'll accept from the model (guards against
/// empty / 1-char junk output).
const MIN_TITLE_LEN: usize = 3;
/// Maximum length in chars before we truncate — typical good titles are
/// 3-7 words, ~50 chars; 100 is a generous upper bound.
const MAX_TITLE_LEN: usize = 100;

/// How much of the plan body to include in the title-generation prompt.
const PLAN_CONTEXT_CHARS: usize = 1000;

/// Keeps the tail of the post-compact conversation so bare `/rename` follows the current topic.
const CONVERSATION_CONTEXT_CHARS: usize = 1_000;

/// Forced-tool name used for session name generation.
pub const SESSION_NAME_TOOL_NAME: &str = "generate_session_name";

/// Build the system + user prompt pair for the title-generation call.
///
/// Returns `(system_prompt, user_prompt)`. Callers pass these to the
/// Fast-role model and parse the response via [`parse_title_response`].
pub fn build_title_prompt(plan_text: &str) -> (String, String) {
    let system = "You generate concise, human-friendly session titles \
        for software-engineering work. Output ONLY a JSON object like \
        {\"title\":\"Fix login button on mobile\"}. The title should be \
        3-7 words, sentence case, no trailing punctuation, no quotes, \
        describing the goal of the plan — not its structure. Never \
        include phrases like 'Plan for' or 'Add feature'; lead with the \
        verb or change."
        .to_string();

    let mut context = plan_text;
    if context.len() > PLAN_CONTEXT_CHARS {
        // Truncate at char boundary to avoid splitting UTF-8 sequences.
        let mut end = PLAN_CONTEXT_CHARS;
        while !context.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        context = &context[..end];
    }
    let user = format!(
        "Generate a title for this plan. Respond with JSON only.\n\n\
         --- PLAN ---\n{context}"
    );
    (system, user)
}

/// Parse the model's JSON response into a title string.
///
/// Accepts either a JSON object `{"title": "..."}` (the strict schema
/// the prompt asks for) or a raw JSON string. Returns `None` if the
/// response doesn't parse or the extracted title is outside length
/// bounds — callers should fall back gracefully (leave the session
/// untitled).
pub fn parse_title_response(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Primary path: strict JSON object.
    #[derive(Deserialize)]
    struct TitleJson {
        title: String,
    }
    if let Ok(parsed) = serde_json::from_str::<TitleJson>(trimmed) {
        return normalize_title(&parsed.title);
    }

    // Fallback: bare JSON string ("Fix login button").
    if let Ok(s) = serde_json::from_str::<String>(trimmed) {
        return normalize_title(&s);
    }

    // Last resort: model ignored the schema and returned plain text.
    // Take the first line, hope for the best.
    let first_line = trimmed.lines().next().unwrap_or("").trim();
    // Strip common wrapping: outer quotes, `title:` labels.
    let cleaned = first_line
        .trim_start_matches("title:")
        .trim_start_matches("Title:")
        .trim_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace());
    normalize_title(cleaned)
}

fn normalize_title(raw: &str) -> Option<String> {
    let s = raw.trim().trim_end_matches(['.', '!']);
    if s.len() < MIN_TITLE_LEN {
        return None;
    }
    let truncated = if s.len() > MAX_TITLE_LEN {
        // Char-boundary safe truncation.
        let mut end = MAX_TITLE_LEN;
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        s[..end].to_string()
    } else {
        s.to_string()
    };
    Some(truncated)
}

/// Apply an LLM-generated title via [`SessionManager::set_ai_title`].
///
/// **Read-precedence note**: titles are persisted as
/// `MetadataEntry::AiTitle`, which the lite-metadata scan
/// (`storage::read_transcript_metadata`) treats as a strict
/// fallback for `CustomTitle`. A subsequent user `/rename` writes
/// `CustomTitle` and wins automatically on the next read, regardless
/// of file ordering — no overwrite guard is required here.
///
/// Returns `true` if the session did not already carry a title
/// (user-set or AI-set) when this writer ran; `false` if a title was
/// already present. Truthy result is advisory — the AiTitle is
/// appended unconditionally so the resume picker can still surface
/// it if the prior title becomes unreadable.
pub fn apply_title(
    manager: &crate::SessionManager,
    session_id: &str,
    title: String,
) -> crate::Result<bool> {
    let session = manager.load(session_id)?;
    let already_titled = session.title.as_deref().is_some_and(|t| !t.is_empty());
    manager.set_ai_title(session_id, &title)?;
    Ok(!already_titled)
}

/// Build the system + user prompt pair for a kebab-case session-name
/// generation call.
///
/// `conversation_text` is the caller-formatted transcript snapshot
/// from [`extract_conversation_text`].
pub fn build_session_name_prompt(conversation_text: &str) -> (String, String) {
    let system = "Generate a short kebab-case name (2-4 words) that captures \
        the main topic of this conversation. Use lowercase words separated \
        by hyphens. Examples: \"fix-login-bug\", \"add-auth-feature\", \
        \"refactor-api-client\", \"debug-test-failures\". Return JSON with \
        a \"name\" field."
        .to_string();

    let user = conversation_text.to_string();
    (system, user)
}

/// JSON Schema shared by native structured output and forced-tool
/// fallback for bare `/rename`.
pub fn session_name_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "description": "A short kebab-case session name, 2-4 words."
            }
        },
        "required": ["name"],
        "additionalProperties": false,
    })
}

/// Parse the model's native structured-output JSON response.
///
/// Strictly accepts only `{"name": "..."}`. Plain text, `name: ...`,
/// and bare JSON strings are intentionally rejected so provider
/// failures fall through to the forced-tool path.
pub fn parse_session_name_response(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    #[derive(Deserialize)]
    struct NameJson {
        name: String,
    }
    serde_json::from_str::<NameJson>(trimmed)
        .ok()
        .and_then(|parsed| clean_session_name(&parsed.name))
}

/// Parse the input object from the forced `generate_session_name` tool.
pub fn parse_session_name_tool_input(input: &serde_json::Value) -> Option<String> {
    #[derive(Deserialize)]
    struct NameJson {
        name: String,
    }
    serde_json::from_value::<NameJson>(input.clone())
        .ok()
        .and_then(|parsed| clean_session_name(&parsed.name))
}

fn clean_session_name(raw: &str) -> Option<String> {
    let out = raw.trim().to_string();
    if out.is_empty() {
        return None;
    }
    Some(out)
}

/// Extract conversation text for bare `/rename`.
///
/// Uses messages after the latest compact boundary, keeps only
/// user-input and assistant text blocks, skips meta/virtual/compact
/// content, omits role prefixes, and returns the last 1000 chars.
pub fn extract_conversation_text<M: std::borrow::Borrow<coco_messages::Message>>(
    messages: &[M],
) -> String {
    let slice = coco_messages::messages_after_compact_boundary(messages);
    let mut parts: Vec<String> = Vec::new();
    for item in slice {
        let msg = item.borrow();
        if coco_messages::predicates::is_meta_message(msg)
            || coco_messages::predicates::is_virtual_message(msg)
            || coco_messages::predicates::is_compact_summary(msg)
        {
            continue;
        }
        match msg {
            coco_messages::Message::User(u) => {
                if u.origin
                    .is_some_and(|origin| origin != coco_messages::MessageOrigin::UserInput)
                {
                    continue;
                }
                if let coco_messages::LlmMessage::User { content, .. } = &u.message {
                    for part in content {
                        if let coco_messages::UserContent::Text(text) = part {
                            let trimmed = text.text.trim();
                            if !trimmed.is_empty() {
                                parts.push(trimmed.to_string());
                            }
                        }
                    }
                }
            }
            coco_messages::Message::Assistant(a) => {
                if let coco_messages::LlmMessage::Assistant { content, .. } = &a.message {
                    for part in content {
                        if let coco_messages::AssistantContent::Text(text) = part {
                            let trimmed = text.text.trim();
                            if !trimmed.is_empty() {
                                parts.push(trimmed.to_string());
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    tail_chars(&parts.join("\n"), CONVERSATION_CONTEXT_CHARS)
}

fn tail_chars(s: &str, max_chars: usize) -> String {
    let total = s.chars().count();
    if total <= max_chars {
        return s.to_string();
    }
    s.chars().skip(total - max_chars).collect()
}

#[cfg(test)]
#[path = "title_generator.test.rs"]
mod tests;
