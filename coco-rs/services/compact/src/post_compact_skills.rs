//! Post-compact skill re-injection.
//!
//! Re-injects invoked skill content at the compact boundary so it survives
//! context compaction. The next-turn `InvokedSkillsGenerator` provides a
//! second injection path for cases where the in-band attachment was
//! budget-clipped.
//!
//! coco-rs deliberately keeps this module loose-coupled: the caller
//! converts whatever skill-state representation it owns
//! (`coco_system_reminder::InvokedSkillEntry`) into [`PostCompactSkill`]
//! and passes a `&[PostCompactSkill]` slice. We never reach into the
//! reminder crate to avoid a `compact → system_reminder` dep.
//!
//! Budgets:
//!   - `POST_COMPACT_MAX_TOKENS_PER_SKILL = 5_000`
//!   - `POST_COMPACT_SKILLS_TOKEN_BUDGET = 25_000`

use coco_messages::AttachmentMessage;
use coco_messages::LlmMessage;

use crate::types::POST_COMPACT_MAX_TOKENS_PER_SKILL;
use crate::types::POST_COMPACT_SKILLS_TOKEN_BUDGET;

/// Caller-supplied view of a skill that should ride through compaction.
#[derive(Debug, Clone)]
pub struct PostCompactSkill {
    /// Display name (e.g. "create-pr").
    pub name: String,
    /// On-disk path of the skill markdown.
    pub path: String,
    /// Body text — will be wrapped in a `<system-reminder>` block.
    pub content: String,
}

/// Build skill attachments for the post-compact bundle.
///
/// Iterates `skills` in order (caller pre-sorts by `invokedAt`), truncates
/// each body at `POST_COMPACT_MAX_TOKENS_PER_SKILL`, and stops adding once
/// the cumulative size hits `POST_COMPACT_SKILLS_TOKEN_BUDGET`.
#[must_use]
pub fn create_post_compact_skill_attachments(
    skills: &[PostCompactSkill],
) -> Vec<AttachmentMessage> {
    let mut used_tokens: i64 = 0;
    let mut out = Vec::new();
    for skill in skills {
        // Truncate the body so a single skill can't blow the per-skill cap.
        let body = truncate_to_tokens(&skill.content, POST_COMPACT_MAX_TOKENS_PER_SKILL);
        let text = format!(
            "Skill `{name}` (from {path}):\n{body}",
            name = skill.name,
            path = skill.path,
            body = body,
        );
        let cost = coco_messages::estimate_text_tokens(&text);
        if used_tokens + cost > POST_COMPACT_SKILLS_TOKEN_BUDGET {
            break;
        }
        used_tokens += cost;
        out.push(AttachmentMessage::api(
            coco_types::AttachmentKind::InvokedSkills,
            LlmMessage::user_text(coco_messages::wrapping::wrap_in_system_reminder(&text)),
        ));
    }
    out
}

/// Greedy character truncation, respecting UTF-8 boundaries, sized so
/// the resulting text estimates at ≤ `max_tokens` per
/// [`crate::tokens::estimate_text_tokens`].
fn truncate_to_tokens(s: &str, max_tokens: i64) -> String {
    if max_tokens <= 0 {
        return String::new();
    }
    let estimated = coco_messages::estimate_text_tokens(s);
    if estimated <= max_tokens {
        return s.to_string();
    }
    // estimate_text_tokens uses ~4 chars / token heuristic; size accordingly
    // and add an ellipsis marker.
    let target_chars = (max_tokens as usize).saturating_mul(4);
    let mut end = target_chars.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = s[..end].to_string();
    out.push_str("\n…[skill truncated for post-compact budget]");
    out
}

#[cfg(test)]
#[path = "post_compact_skills.test.rs"]
mod tests;
