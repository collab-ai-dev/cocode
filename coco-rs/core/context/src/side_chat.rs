//! Sidechat context contracts: budgets, the bounded-inheritance container, and
//! the boundary fragment inserted between inherited reference context and the
//! child's first question.
//!
//! Token estimation is never reimplemented here — it reuses
//! `coco_messages::token_estimation`. See
//! `docs/internal/sidechat-architecture.md`.

use std::sync::Arc;

use coco_messages::Message;
use coco_messages::estimate_text_tokens;
use coco_types::MessageOrigin;

/// Max estimated tokens for any single inherited fragment.
pub const MAX_TOKENS_PER_INHERITED_FRAGMENT: i64 = 8_192;

/// Absolute cap on total inherited tokens, before the per-model halving.
pub const MAX_INHERITED_TOKENS_ABSOLUTE_CAP: i64 = 32_768;

/// Tokens always reserved for the boundary, question, tools, and response.
pub const MIN_RESERVED_TOKENS: i64 = 8_192;

/// Total inherited-token budget for a model whose context window is
/// `model_context_window`.
///
/// At most half of the window may be inherited, the absolute cap still
/// applies, and [`MIN_RESERVED_TOKENS`] is always left for the boundary,
/// question, tools, and response.
pub fn max_inherited_tokens(model_context_window: i64) -> i64 {
    MAX_INHERITED_TOKENS_ABSOLUTE_CAP
        .min((model_context_window / 2).max(0))
        .min((model_context_window - MIN_RESERVED_TOKENS).max(0))
}

/// A bounded, user-visible context fragment inserted into a prompt exactly once.
/// Implementors render deterministic, budget-bounded text.
pub trait ContextualUserFragment {
    /// Render the fragment's text.
    fn render(&self) -> String;

    /// Estimated token cost of the rendered text. Reuses coco-messages so the
    /// estimate matches history accounting.
    fn estimated_tokens(&self) -> i64 {
        estimate_text_tokens(&self.render())
    }
}

/// Fidelity of a captured [`BoundedContext`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextFidelity {
    /// The complete committed parent prefix fit within budget, verbatim.
    FullPrefix,
    /// Only the newest complete conversation groups fit; a typed omission marker
    /// was prepended and no parent cache hit is claimed.
    BoundedFallback { omitted_groups: usize },
}

impl ContextFidelity {
    /// True when no committed parent message was omitted.
    pub fn is_full_prefix(&self) -> bool {
        matches!(self, ContextFidelity::FullPrefix)
    }
}

/// Inherited parent context, already reduced to fit the sidechat budget. Never
/// splits a UTF-8 string or a tool-use/tool-result group.
#[derive(Debug, Clone)]
pub struct BoundedContext {
    /// Inherited messages, oldest first. Complete groups only.
    messages: Vec<Arc<Message>>,
    /// Estimated total tokens of `messages`.
    estimated_tokens: i64,
    /// Whether this is the full parent prefix or a bounded fallback.
    fidelity: ContextFidelity,
}

impl BoundedContext {
    /// An empty inheritance (no parent context carried).
    pub fn empty() -> Self {
        Self {
            messages: Vec::new(),
            estimated_tokens: 0,
            fidelity: ContextFidelity::FullPrefix,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    pub fn messages(&self) -> &[Arc<Message>] {
        &self.messages
    }

    pub fn estimated_tokens(&self) -> i64 {
        self.estimated_tokens
    }

    pub fn fidelity(&self) -> &ContextFidelity {
        &self.fidelity
    }

    pub fn into_messages(self) -> Vec<Arc<Message>> {
        self.messages
    }
}

/// The boundary fragment placed after inherited reference context and
/// immediately before the child's first question. Its text states only facts
/// the runtime enforces (see the design doc §8.3).
#[derive(Debug, Clone, Default)]
pub struct SideChatBoundaryFragment;

/// Rendered boundary text. Bounded and constant.
const SIDE_CHAT_BOUNDARY_TEXT: &str = "<system-reminder>You are answering a side question. The messages above are reference material inherited from a parent conversation.

- You do not modify the parent conversation; nothing you do here changes it.
- Your tools are restricted to read-only inspection (Read, Glob, Grep, and read-only shell commands). Any other tool call is denied.
- Normal permission rules still apply, so a read may still require approval.

Answer the question below using this context.</system-reminder>";

impl ContextualUserFragment for SideChatBoundaryFragment {
    fn render(&self) -> String {
        SIDE_CHAT_BOUNDARY_TEXT.to_string()
    }
}

/// Marker prepended when older parent context had to be omitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SideChatOmissionFragment {
    omitted_groups: usize,
}

impl SideChatOmissionFragment {
    pub fn new(omitted_groups: usize) -> Self {
        Self { omitted_groups }
    }
}

impl ContextualUserFragment for SideChatOmissionFragment {
    fn render(&self) -> String {
        let omitted_groups = self.omitted_groups;
        format!(
            "<system-reminder>Earlier parent-conversation context was omitted to fit the sidechat context budget ({omitted_groups} complete conversation groups omitted). Treat the retained messages below as a suffix, not the full conversation.</system-reminder>"
        )
    }
}

fn starts_semantic_user_turn(message: &Message) -> bool {
    let Message::User(user) = message else {
        return false;
    };
    if user.is_virtual || user.parent_tool_use_id.is_some() || user.is_visible_in_transcript_only {
        return false;
    }
    !matches!(user.origin, Some(MessageOrigin::SystemInjected))
}

fn message_tokens(message: &Arc<Message>) -> i64 {
    coco_messages::estimate_tokens_for_messages(std::slice::from_ref(message))
}

fn fragments_fit(messages: &[Arc<Message>]) -> bool {
    messages
        .iter()
        .all(|message| message_tokens(message) <= MAX_TOKENS_PER_INHERITED_FRAGMENT)
}

fn omission_message(omitted_groups: usize) -> Arc<Message> {
    Arc::new(coco_messages::create_meta_message(
        &SideChatOmissionFragment::new(omitted_groups).render(),
    ))
}

fn omitted_group_count(group_starts: &[usize], chosen: usize) -> usize {
    group_starts.iter().filter(|&&start| start < chosen).count()
        + usize::from(group_starts.first().is_some_and(|&start| start > 0))
}

/// Reduce committed parent history to fit `max_inherited_tokens`, never
/// splitting a conversation group — a `User` turn and everything through the
/// message before the next `User`.
///
/// - Under budget → the complete committed prefix verbatim
///   ([`ContextFidelity::FullPrefix`]).
/// - Over budget → the newest whole groups that fit, tagged
///   [`ContextFidelity::BoundedFallback`] with the count of dropped older turns.
/// - If the single newest turn already exceeds the whole budget (or there is no
///   `User` boundary to truncate at), returns
///   [`crate::ContextError::SideChatContextTooLarge`] with a `/compact` hint.
///
/// The kept slice always begins at a `User` boundary, so a tool-use / tool-result
/// pair is never severed.
pub fn capture_bounded_context(
    messages: &[Arc<Message>],
    max_inherited_tokens: i64,
) -> Result<BoundedContext, crate::ContextError> {
    let total = coco_messages::estimate_tokens_for_messages(messages);
    if total <= max_inherited_tokens && fragments_fit(messages) {
        return Ok(BoundedContext {
            messages: messages.to_vec(),
            estimated_tokens: total,
            fidelity: ContextFidelity::FullPrefix,
        });
    }

    let group_starts: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, message)| starts_semantic_user_turn(message.as_ref()))
        .map(|(i, _)| i)
        .collect();

    let Some(&newest_start) = group_starts.last() else {
        return Err(crate::ContextError::side_chat_context_too_large(
            "sidechat inherited context exceeds the budget and has no user-turn boundary to \
             truncate at — run /compact in the main conversation first",
        ));
    };
    let newest = &messages[newest_start..];
    let newest_marker = omission_message(omitted_group_count(&group_starts, newest_start).max(1));
    let newest_total =
        message_tokens(&newest_marker) + coco_messages::estimate_tokens_for_messages(newest);
    if !fragments_fit(newest) || newest_total > max_inherited_tokens {
        return Err(crate::ContextError::side_chat_context_too_large(
            "the most recent conversation turn contains a fragment larger than 8,192 tokens or \
             exceeds the entire sidechat context budget — run /compact in the main conversation \
             first",
        ));
    }

    // Largest suffix that starts at a semantic user-turn boundary and still
    // fits after accounting for the required omission marker.
    let mut chosen = newest_start;
    for &start in group_starts.iter().rev() {
        let omitted_groups = omitted_group_count(&group_starts, start);
        let marker = omission_message(omitted_groups.max(1));
        let candidate = &messages[start..];
        let candidate_total =
            message_tokens(&marker) + coco_messages::estimate_tokens_for_messages(candidate);
        if fragments_fit(candidate) && candidate_total <= max_inherited_tokens {
            chosen = start;
        } else {
            break;
        }
    }

    let omitted_groups = omitted_group_count(&group_starts, chosen);
    let mut kept = Vec::with_capacity(messages.len() - chosen + 1);
    kept.push(omission_message(omitted_groups.max(1)));
    kept.extend_from_slice(&messages[chosen..]);
    let estimated_tokens = coco_messages::estimate_tokens_for_messages(&kept);
    Ok(BoundedContext {
        messages: kept,
        estimated_tokens,
        fidelity: ContextFidelity::BoundedFallback {
            omitted_groups: omitted_groups.max(1),
        },
    })
}

#[cfg(test)]
#[path = "side_chat.test.rs"]
mod tests;
